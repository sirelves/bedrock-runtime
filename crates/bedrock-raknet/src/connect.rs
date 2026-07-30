//! Connection opening: MTU discovery, still unframed and in the clear.
//!
//! ```text
//! client  OpenConnectionRequest1  ──►  padded to the MTU being probed
//!         OpenConnectionReply1    ◄──  server GUID, security flag, agreed MTU
//!         OpenConnectionRequest2  ──►  server address, MTU, client GUID
//!         OpenConnectionReply2    ◄──  client address as the server sees it
//! ```
//!
//! When the server sets its security flag it also sends a cookie, which the client
//! must echo in request 2. That is RakNet's answer to being used as a UDP reflector:
//! a spoofed source address never receives the cookie, so it cannot get past request 1.
//! See `SECURITY.md`.
//!
//! A protocol version mismatch is answered with [`IncompatibleProtocolVersion`], which
//! carries the version the server does speak — the protocol tells you how to talk to
//! it, so the probe asks instead of assuming.

use crate::address;
use crate::wire::{DecodeError, Reader, Writer};
use crate::{MAGIC, MAX_MTU};
use std::fmt;
use std::net::SocketAddr;

/// `OpenConnectionRequest1`.
pub const ID_OPEN_CONNECTION_REQUEST_1: u8 = 0x05;
/// `OpenConnectionReply1`.
pub const ID_OPEN_CONNECTION_REPLY_1: u8 = 0x06;
/// `OpenConnectionRequest2`.
pub const ID_OPEN_CONNECTION_REQUEST_2: u8 = 0x07;
/// `OpenConnectionReply2`.
pub const ID_OPEN_CONNECTION_REPLY_2: u8 = 0x08;
/// `IncompatibleProtocolVersion`.
pub const ID_INCOMPATIBLE_PROTOCOL_VERSION: u8 = 0x19;

/// RakNet protocol version spoken by current Bedrock clients.
pub const PROTOCOL_VERSION: u8 = 11;

/// Why an opening packet did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectError {
    /// The buffer ended early.
    Truncated(DecodeError),
    /// The first byte was not the expected packet id.
    UnexpectedId {
        /// The id required.
        expected: u8,
        /// The id found.
        found: u8,
    },
    /// The 16-byte constant was missing or wrong.
    BadMagic,
    /// An embedded address did not decode.
    Address(address::AddressError),
}

impl From<DecodeError> for ConnectError {
    fn from(e: DecodeError) -> Self {
        Self::Truncated(e)
    }
}

impl From<address::AddressError> for ConnectError {
    fn from(e: address::AddressError) -> Self {
        Self::Address(e)
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(e) => write!(f, "{e}"),
            Self::UnexpectedId { expected, found } => {
                write!(f, "expected packet id {expected:#04x}, found {found:#04x}")
            }
            Self::BadMagic => write!(f, "RakNet magic missing or wrong"),
            Self::Address(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ConnectError {}

fn expect(r: &mut Reader<'_>, id: u8) -> Result<(), ConnectError> {
    let found = r.u8()?;
    if found != id {
        return Err(ConnectError::UnexpectedId {
            expected: id,
            found,
        });
    }
    if r.array::<16>()? != MAGIC {
        return Err(ConnectError::BadMagic);
    }
    Ok(())
}

/// Builds `OpenConnectionRequest1`, padded so the whole datagram is `mtu` bytes.
///
/// The padding is the probe: the datagram either arrives at that size or it does not,
/// and the server replies with the largest MTU it saw. Returns `None` if `mtu` is
/// smaller than the fixed part of the packet.
pub fn encode_request_1(mtu: usize) -> Option<Vec<u8>> {
    const FIXED: usize = 1 + MAGIC.len() + 1;
    let padding = mtu.checked_sub(FIXED)?;
    let mut w = Writer::new();
    w.u8(ID_OPEN_CONNECTION_REQUEST_1)
        .bytes(&MAGIC)
        .u8(PROTOCOL_VERSION)
        .zeros(padding);
    Some(w.finish())
}

/// A decoded `OpenConnectionReply1`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply1 {
    /// The server's GUID.
    pub server_guid: i64,
    /// Cookie to echo in request 2, present when the server enables security.
    pub cookie: Option<u32>,
    /// The MTU the server agrees to.
    pub mtu: u16,
}

/// Decodes `OpenConnectionReply1`.
pub fn decode_reply_1(buf: &[u8]) -> Result<Reply1, ConnectError> {
    let mut r = Reader::new(buf);
    expect(&mut r, ID_OPEN_CONNECTION_REPLY_1)?;
    let server_guid = r.i64()?;
    let cookie = if r.u8()? != 0 { Some(r.u32()?) } else { None };
    Ok(Reply1 {
        server_guid,
        cookie,
        mtu: r.u16()?,
    })
}

/// Builds `OpenConnectionRequest2`. `cookie` must be whatever [`Reply1`] carried.
pub fn encode_request_2(
    server: SocketAddr,
    mtu: u16,
    client_guid: i64,
    cookie: Option<u32>,
) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(ID_OPEN_CONNECTION_REQUEST_2).bytes(&MAGIC);
    if let Some(cookie) = cookie {
        w.u32(cookie).u8(0);
    }
    address::write(&mut w, server);
    w.u16(mtu).i64(client_guid);
    w.finish()
}

/// A decoded `OpenConnectionReply2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply2 {
    /// The server's GUID.
    pub server_guid: i64,
    /// Our address, as the server sees it.
    pub client_addr: SocketAddr,
    /// The MTU both sides will use from here on.
    pub mtu: u16,
    /// Whether the server enabled encryption at the RakNet layer. Bedrock does not.
    pub encryption_enabled: bool,
}

/// Decodes `OpenConnectionReply2`.
pub fn decode_reply_2(buf: &[u8]) -> Result<Reply2, ConnectError> {
    let mut r = Reader::new(buf);
    expect(&mut r, ID_OPEN_CONNECTION_REPLY_2)?;
    Ok(Reply2 {
        server_guid: r.i64()?,
        client_addr: address::read(&mut r)?,
        mtu: r.u16()?,
        encryption_enabled: r.u8()? != 0,
    })
}

/// A decoded `IncompatibleProtocolVersion`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompatibleProtocolVersion {
    /// The RakNet protocol version the server speaks.
    pub server_protocol: u8,
    /// The server's GUID.
    pub server_guid: i64,
}

/// Decodes `IncompatibleProtocolVersion`.
pub fn decode_incompatible(buf: &[u8]) -> Result<IncompatibleProtocolVersion, ConnectError> {
    let mut r = Reader::new(buf);
    let found = r.u8()?;
    if found != ID_INCOMPATIBLE_PROTOCOL_VERSION {
        return Err(ConnectError::UnexpectedId {
            expected: ID_INCOMPATIBLE_PROTOCOL_VERSION,
            found,
        });
    }
    let server_protocol = r.u8()?;
    if r.array::<16>()? != MAGIC {
        return Err(ConnectError::BadMagic);
    }
    Ok(IncompatibleProtocolVersion {
        server_protocol,
        server_guid: r.i64()?,
    })
}

/// MTU sizes to probe, largest first.
///
/// A server answers with the size of the largest request 1 that reached it, so the
/// walk down exists for paths that silently drop anything bigger.
pub const MTU_LADDER: [usize; 3] = [MAX_MTU, 1200, 576];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_1_is_padded_to_the_probed_mtu() {
        for mtu in MTU_LADDER {
            let buf = encode_request_1(mtu).unwrap();
            assert_eq!(buf.len(), mtu);
            assert_eq!(buf[0], ID_OPEN_CONNECTION_REQUEST_1);
            assert_eq!(buf[17], PROTOCOL_VERSION);
        }
    }

    #[test]
    fn request_1_refuses_an_mtu_smaller_than_itself() {
        assert_eq!(encode_request_1(4), None);
    }

    fn reply_1(cookie: Option<u32>) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(ID_OPEN_CONNECTION_REPLY_1).bytes(&MAGIC).i64(7);
        match cookie {
            Some(c) => {
                w.u8(1).u32(c);
            }
            None => {
                w.u8(0);
            }
        }
        w.u16(1400);
        w.finish()
    }

    #[test]
    fn reply_1_without_security_has_no_cookie() {
        let r = decode_reply_1(&reply_1(None)).unwrap();
        assert_eq!(r.server_guid, 7);
        assert_eq!(r.cookie, None);
        assert_eq!(r.mtu, 1400);
    }

    /// The cookie shifts the MTU field, so reading it wrong yields a wrong MTU rather
    /// than a decode failure.
    #[test]
    fn reply_1_with_security_carries_a_cookie() {
        let r = decode_reply_1(&reply_1(Some(0xdead_beef))).unwrap();
        assert_eq!(r.cookie, Some(0xdead_beef));
        assert_eq!(r.mtu, 1400);
    }

    #[test]
    fn request_2_grows_by_the_cookie() {
        let addr = "1.2.3.4:19132".parse().unwrap();
        let plain = encode_request_2(addr, 1400, 9, None);
        let with_cookie = encode_request_2(addr, 1400, 9, Some(1));
        assert_eq!(with_cookie.len(), plain.len() + 5);
    }

    #[test]
    fn reply_2_round_trips() {
        let client: SocketAddr = "203.0.113.5:41234".parse().unwrap();
        let mut w = Writer::new();
        w.u8(ID_OPEN_CONNECTION_REPLY_2).bytes(&MAGIC).i64(-3);
        address::write(&mut w, client);
        w.u16(1400).u8(0);

        let r = decode_reply_2(&w.finish()).unwrap();
        assert_eq!(r.server_guid, -3);
        assert_eq!(r.client_addr, client);
        assert_eq!(r.mtu, 1400);
        assert!(!r.encryption_enabled);
    }

    #[test]
    fn incompatible_version_reports_what_the_server_speaks() {
        let mut w = Writer::new();
        w.u8(ID_INCOMPATIBLE_PROTOCOL_VERSION)
            .u8(11)
            .bytes(&MAGIC)
            .i64(42);
        let r = decode_incompatible(&w.finish()).unwrap();
        assert_eq!(r.server_protocol, 11);
        assert_eq!(r.server_guid, 42);
    }

    #[test]
    fn wrong_magic_is_rejected() {
        let mut buf = reply_1(None);
        buf[5] ^= 0xff;
        assert_eq!(decode_reply_1(&buf), Err(ConnectError::BadMagic));
    }

    #[test]
    fn wrong_id_is_rejected() {
        let mut buf = reply_1(None);
        buf[0] = ID_OPEN_CONNECTION_REPLY_2;
        assert!(matches!(
            decode_reply_1(&buf),
            Err(ConnectError::UnexpectedId { .. })
        ));
    }

    #[test]
    fn truncated_replies_fail_cleanly() {
        let full = reply_1(None);
        for n in 0..full.len() {
            assert!(decode_reply_1(&full[..n]).is_err(), "prefix of {n} bytes");
        }
    }
}
