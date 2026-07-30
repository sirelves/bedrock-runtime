//! The batch: how game packets are framed inside one RakNet payload.
//!
//! ```text
//! fe                    marker
//! varint len | packet   repeated until the payload ends
//! ```
//!
//! Each packet starts with a varint header carrying the id in its low ten bits and two
//! subclient fields above it — split-screen players share one connection, and reading
//! the header as a plain id would decode a second player's packets as garbage ids.
//!
//! **The shape changes once `NetworkSettings` has been sent.** Before it, packets
//! follow the marker directly. After it, a method byte sits between them, even when
//! the negotiated method is no compression at all:
//!
//! ```text
//! before   fe | varint len | packet ...
//! after    fe | method | varint len | packet ...
//! ```
//!
//! Observed, not inferred: a real client opened with `fe 06 c1 01 ...` and then, once
//! told which compression to use, sent its login as `fe ff f1 f1 23 01 ...`. Decoding
//! the second with the first shape reads the method byte as a length and everything
//! after it is garbage.

use crate::bytes::{DecodeError, Reader, Writer};

/// First byte of every batch.
pub const MARKER: u8 = 0xfe;

/// The compression method byte carried by batches sent after `NetworkSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// Raw deflate. Byte value follows Mojang's algorithm enum; not yet observed.
    ZLib = 0x00,
    /// Snappy. Byte value follows Mojang's algorithm enum; not yet observed.
    Snappy = 0x01,
    /// Not compressed. Observed from a real client answering `Compression::None`.
    None = 0xff,
}

impl Method {
    /// Reads the method byte.
    pub fn from_byte(byte: u8) -> Option<Self> {
        Some(match byte {
            0x00 => Self::ZLib,
            0x01 => Self::Snappy,
            0xff => Self::None,
            _ => return None,
        })
    }
}

const ID_BITS: u32 = 0x3ff;
const SENDER_SHIFT: u32 = 10;
const TARGET_SHIFT: u32 = 12;
const SUBCLIENT_BITS: u32 = 0x3;

/// One packet inside a batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// Packet id, from the low ten bits of the header.
    pub id: u32,
    /// Which split-screen player sent it.
    pub sender: u8,
    /// Which split-screen player it is for.
    pub target: u8,
    /// The body, without the header.
    pub body: Vec<u8>,
}

impl Packet {
    /// A packet from the primary player to the primary player.
    pub fn new(id: u32, body: Vec<u8>) -> Self {
        Self {
            id,
            sender: 0,
            target: 0,
            body,
        }
    }

    fn header(&self) -> u32 {
        (self.id & ID_BITS)
            | (u32::from(self.sender) & SUBCLIENT_BITS) << SENDER_SHIFT
            | (u32::from(self.target) & SUBCLIENT_BITS) << TARGET_SHIFT
    }
}

/// Wraps packets in a batch, uncompressed.
pub fn encode(packets: &[Packet]) -> Vec<u8> {
    let mut out = Writer::new();
    out.u8(MARKER);
    for packet in packets {
        let mut inner = Writer::new();
        inner.varint(packet.header()).bytes(&packet.body);
        out.prefixed(&inner.finish());
    }
    out.finish()
}

/// Wraps packets in a batch carrying a method byte, for use after `NetworkSettings`.
///
/// Only [`Method::None`] is supported: the bytes still go out uncompressed, and the
/// byte says so. Asking for a real algorithm without implementing it would produce a
/// batch no client can read.
pub fn encode_with_method(packets: &[Packet]) -> Vec<u8> {
    let body = encode(packets);
    let mut out = Writer::new();
    out.u8(MARKER).u8(Method::None as u8).bytes(&body[1..]);
    out.finish()
}

/// Splits a batch sent before `NetworkSettings`, which has no method byte.
pub fn decode(bytes: &[u8]) -> Result<Vec<Packet>, DecodeError> {
    let mut r = Reader::new(bytes);
    if r.u8()? != MARKER {
        return Err(DecodeError::UnexpectedEnd {
            needed: 1,
            available: 0,
        });
    }
    decode_packets(r)
}

/// Splits a batch sent after `NetworkSettings`, reading its method byte.
///
/// Returns the method alongside the packets so a caller can tell an uncompressed batch
/// from one it cannot yet read, rather than failing the same way for both.
pub fn decode_with_method(bytes: &[u8]) -> Result<(Method, Vec<Packet>), DecodeError> {
    let mut r = Reader::new(bytes);
    if r.u8()? != MARKER {
        return Err(DecodeError::UnexpectedEnd {
            needed: 1,
            available: 0,
        });
    }
    let byte = r.u8()?;
    let Some(method) = Method::from_byte(byte) else {
        return Err(DecodeError::UnexpectedEnd {
            needed: 1,
            available: 0,
        });
    };
    if method != Method::None {
        // Nothing decompresses yet; say which method rather than pretending to parse.
        return Ok((method, Vec::new()));
    }
    Ok((method, decode_packets(r)?))
}

/// Splits the packet stream that follows the marker and any compression header.
pub fn decode_packets(mut r: Reader<'_>) -> Result<Vec<Packet>, DecodeError> {
    let mut packets = Vec::new();
    while !r.is_empty() {
        let mut inner = Reader::new(r.prefixed()?);
        let header = inner.varint()?;
        let body = inner.bytes(inner.remaining())?.to_vec();
        packets.push(Packet {
            id: header & ID_BITS,
            sender: ((header >> SENDER_SHIFT) & SUBCLIENT_BITS) as u8,
            target: ((header >> TARGET_SHIFT) & SUBCLIENT_BITS) as u8,
            body,
        });
    }
    Ok(packets)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_batch_round_trips() {
        let packets = vec![
            Packet::new(193, vec![0, 0, 3, 233]),
            Packet::new(143, vec![1]),
        ];
        assert_eq!(decode(&encode(&packets)).unwrap(), packets);
    }

    /// The bytes a real client sent, decoded by the general path.
    #[test]
    fn the_captured_login_opener_decodes() {
        let raw = [0xfe, 0x06, 0xc1, 0x01, 0x00, 0x00, 0x03, 0xe9];
        let packets = decode(&raw).unwrap();
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].id, 193);
        assert_eq!(packets[0].sender, 0);
        assert_eq!(packets[0].body, vec![0x00, 0x00, 0x03, 0xe9]);
    }

    /// And our encoder reproduces them exactly.
    #[test]
    fn we_encode_what_the_client_encoded() {
        let encoded = encode(&[Packet::new(193, vec![0x00, 0x00, 0x03, 0xe9])]);
        assert_eq!(
            encoded,
            vec![0xfe, 0x06, 0xc1, 0x01, 0x00, 0x00, 0x03, 0xe9]
        );
    }

    /// Split-screen players share a connection. Ignoring the subclient bits would turn
    /// a second player's packet id into a different, wrong one.
    #[test]
    fn subclient_fields_survive_the_header() {
        let packet = Packet {
            id: 193,
            sender: 2,
            target: 1,
            body: vec![7],
        };
        let decoded = decode(&encode(std::slice::from_ref(&packet))).unwrap();
        assert_eq!(decoded[0], packet);
    }

    #[test]
    fn an_empty_batch_decodes_to_nothing() {
        assert_eq!(decode(&[MARKER]).unwrap(), vec![]);
    }

    #[test]
    fn a_packet_with_no_body_is_fine() {
        let packets = vec![Packet::new(5, vec![])];
        assert_eq!(decode(&encode(&packets)).unwrap(), packets);
    }

    #[test]
    fn a_length_beyond_the_batch_fails_cleanly() {
        let raw = [0xfe, 0x40, 0xc1, 0x01];
        assert!(decode(&raw).is_err());
    }

    #[test]
    fn a_missing_marker_is_refused() {
        assert!(decode(&[0x00, 0x01, 0x02]).is_err());
        assert!(decode(&[]).is_err());
    }

    /// The exact framing a real client used for its login, once told to use no
    /// compression. Decoding this with the pre-settings shape reads 0xff as a length.
    #[test]
    fn the_captured_login_framing_decodes() {
        // 0x05: one byte of header plus four of body.
        let raw = [0xfe, 0xff, 0x05, 0x01, 0x00, 0x00, 0x03, 0xe9];
        let (method, packets) = decode_with_method(&raw).unwrap();
        assert_eq!(method, Method::None);
        assert_eq!(packets.len(), 1);
        assert_eq!(packets[0].id, 1, "packet 1 is Login");
        assert_eq!(packets[0].body, vec![0x00, 0x00, 0x03, 0xe9]);

        assert!(
            decode(&raw).map(|p| p[0].id) != Ok(1),
            "the pre-settings shape must not accidentally agree"
        );
    }

    #[test]
    fn a_method_batch_round_trips() {
        let packets = vec![Packet::new(1, vec![1, 2, 3])];
        let (method, decoded) = decode_with_method(&encode_with_method(&packets)).unwrap();
        assert_eq!(method, Method::None);
        assert_eq!(decoded, packets);
    }

    /// A method we cannot decompress is named rather than mistaken for corruption.
    #[test]
    fn an_unreadable_method_is_reported() {
        for byte in [0x00u8, 0x01] {
            let raw = [MARKER, byte, 0x99, 0x99];
            let (method, packets) = decode_with_method(&raw).unwrap();
            assert_ne!(method, Method::None);
            assert!(packets.is_empty());
        }
    }

    #[test]
    fn an_unknown_method_byte_is_refused() {
        assert!(decode_with_method(&[MARKER, 0x42, 0x00]).is_err());
    }

    #[test]
    fn truncated_batches_never_panic() {
        let full = encode(&[Packet::new(193, vec![1, 2, 3, 4]), Packet::new(1, vec![9])]);
        for n in 0..full.len() {
            let _ = decode(&full[..n]);
        }
    }
}
