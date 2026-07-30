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
//! Compression sits between the marker and the packets once `NetworkSettings` has
//! negotiated it. Until then, and whenever the negotiated algorithm is
//! [`Compression::None`], the packets follow the marker directly — which is what makes
//! it possible to read a login without implementing zlib.

use crate::bytes::{DecodeError, Reader, Writer};

/// First byte of every batch.
pub const MARKER: u8 = 0xfe;

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

/// Splits an uncompressed batch into its packets.
///
/// `bytes` must already have any compression removed, and must start with [`MARKER`].
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

    #[test]
    fn truncated_batches_never_panic() {
        let full = encode(&[Packet::new(193, vec![1, 2, 3, 4]), Packet::new(1, vec![9])]);
        for n in 0..full.len() {
            let _ = decode(&full[..n]);
        }
    }
}
