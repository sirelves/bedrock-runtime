//! The offline phase: the packets exchanged before a connection exists.
//!
//! These travel in the clear and unframed — no sequence numbers, no reliability, no
//! encryption. That is what makes them usable as a first foothold: the pong answers
//! which protocol version a server speaks without any of the rest of the stack existing.
//!
//! Only ping and pong live here so far. MTU negotiation
//! (`OpenConnectionRequest1/2`) is the next slice — see M0.2 in `docs/ROADMAP.md`.

use crate::MAGIC;
use crate::wire::{DecodeError, Reader, Writer};
use std::fmt;

/// Packet id of `UnconnectedPing`.
pub const ID_UNCONNECTED_PING: u8 = 0x01;

/// Packet id of `UnconnectedPong`.
pub const ID_UNCONNECTED_PONG: u8 = 0x1c;

/// Why a datagram was not the offline packet we expected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OfflineError {
    /// The buffer ended early.
    Truncated(DecodeError),
    /// The first byte was not the expected packet id.
    UnexpectedId {
        /// The id we required.
        expected: u8,
        /// The id actually present.
        found: u8,
    },
    /// The 16-byte constant was missing or wrong, so this is not RakNet.
    BadMagic,
    /// The advertisement string was not valid UTF-8.
    InvalidUtf8,
}

impl From<DecodeError> for OfflineError {
    fn from(e: DecodeError) -> Self {
        Self::Truncated(e)
    }
}

impl fmt::Display for OfflineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(e) => write!(f, "{e}"),
            Self::UnexpectedId { expected, found } => {
                write!(f, "expected packet id {expected:#04x}, found {found:#04x}")
            }
            Self::BadMagic => write!(f, "RakNet magic missing or wrong"),
            Self::InvalidUtf8 => write!(f, "advertisement string is not valid UTF-8"),
        }
    }
}

impl std::error::Error for OfflineError {}

/// Builds an `UnconnectedPing`.
///
/// `time` is echoed back by the server, so the round trip can be measured against a
/// clock we control; `client_guid` identifies this sender. Neither is authenticated —
/// this exchange happens before anything is.
pub fn encode_unconnected_ping(time: i64, client_guid: i64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(ID_UNCONNECTED_PING)
        .i64(time)
        .bytes(&MAGIC)
        .i64(client_guid);
    w.finish()
}

/// A decoded `UnconnectedPong`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnconnectedPong {
    /// The `time` from our ping, echoed back.
    pub time: i64,
    /// The server's GUID.
    pub server_guid: i64,
    /// The raw advertisement string — semicolon-separated fields carrying the MOTD,
    /// protocol version, player counts and more.
    ///
    /// Kept raw on purpose. Splitting it is a separate concern, because the field
    /// layout is a Bedrock convention rather than part of RakNet, and it is one of
    /// the things `docs/PROTOCOL.md` marks as needing confirmation against a real
    /// server.
    pub advertisement: String,
}

/// Decodes an `UnconnectedPong`.
pub fn decode_unconnected_pong(buf: &[u8]) -> Result<UnconnectedPong, OfflineError> {
    let mut r = Reader::new(buf);

    let id = r.u8()?;
    if id != ID_UNCONNECTED_PONG {
        return Err(OfflineError::UnexpectedId {
            expected: ID_UNCONNECTED_PONG,
            found: id,
        });
    }

    let time = r.i64()?;
    let server_guid = r.i64()?;

    if r.array::<16>()? != MAGIC {
        return Err(OfflineError::BadMagic);
    }

    // The length is attacker-controlled, but `bytes` only borrows — a lie here costs
    // us a failed read, not an allocation.
    let len = usize::from(r.u16()?);
    let advertisement =
        std::str::from_utf8(r.bytes(len)?).map_err(|_| OfflineError::InvalidUtf8)?;

    Ok(UnconnectedPong {
        time,
        server_guid,
        advertisement: advertisement.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a pong the way we currently believe servers build one.
    ///
    /// This is a *hypothesis*, not evidence: `docs/PROTOCOL.md` requires that protocol
    /// claims be proven against real bytes. It is replaced by a captured fixture in
    /// `tests/fixtures/` once a live server has answered — until then these tests prove
    /// internal consistency and the error paths, and nothing about Mojang's servers.
    fn synthetic_pong(advertisement: &str) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(ID_UNCONNECTED_PONG).i64(42).i64(-7).bytes(&MAGIC);
        let bytes = advertisement.as_bytes();
        let len = u16::try_from(bytes.len()).expect("test string fits in u16");
        w.bytes(&len.to_be_bytes()).bytes(bytes);
        w.finish()
    }

    #[test]
    fn ping_has_the_expected_shape() {
        let ping = encode_unconnected_ping(1, 2);
        assert_eq!(ping.len(), 1 + 8 + 16 + 8);
        assert_eq!(ping[0], ID_UNCONNECTED_PING);
        assert_eq!(&ping[9..25], &MAGIC);
    }

    #[test]
    fn decodes_a_pong() {
        let pong = decode_unconnected_pong(&synthetic_pong("MCPE;hello;800;1.21.0")).unwrap();
        assert_eq!(pong.time, 42);
        assert_eq!(pong.server_guid, -7);
        assert_eq!(pong.advertisement, "MCPE;hello;800;1.21.0");
    }

    #[test]
    fn empty_advertisement_is_valid() {
        let pong = decode_unconnected_pong(&synthetic_pong("")).unwrap();
        assert_eq!(pong.advertisement, "");
    }

    #[test]
    fn rejects_wrong_packet_id() {
        let mut buf = synthetic_pong("x");
        buf[0] = ID_UNCONNECTED_PING;
        assert_eq!(
            decode_unconnected_pong(&buf),
            Err(OfflineError::UnexpectedId {
                expected: ID_UNCONNECTED_PONG,
                found: ID_UNCONNECTED_PING,
            })
        );
    }

    #[test]
    fn rejects_bad_magic() {
        let mut buf = synthetic_pong("x");
        buf[20] ^= 0xff;
        assert_eq!(decode_unconnected_pong(&buf), Err(OfflineError::BadMagic));
    }

    /// A length that overshoots the buffer must fail as a short read, not allocate.
    #[test]
    fn rejects_length_longer_than_buffer() {
        let mut buf = synthetic_pong("x");
        let n = buf.len();
        buf[n - 3..n - 1].copy_from_slice(&u16::MAX.to_be_bytes());
        assert!(matches!(
            decode_unconnected_pong(&buf),
            Err(OfflineError::Truncated(_))
        ));
    }

    #[test]
    fn rejects_invalid_utf8() {
        let mut buf = synthetic_pong("xx");
        let n = buf.len();
        buf[n - 2..].copy_from_slice(&[0xff, 0xfe]);
        assert_eq!(
            decode_unconnected_pong(&buf),
            Err(OfflineError::InvalidUtf8)
        );
    }

    #[test]
    fn rejects_empty_datagram() {
        assert!(matches!(
            decode_unconnected_pong(&[]),
            Err(OfflineError::Truncated(_))
        ));
    }
}
