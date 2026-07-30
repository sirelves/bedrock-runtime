//! Packets carried inside frames, once the connection is open.
//!
//! `ConnectionRequestAccepted` and `NewIncomingConnection` both carry a run of address
//! slots whose length is not on the wire: RakNet compiles in ten, several Bedrock
//! servers send twenty. Decoding reads slots until only the two trailing timestamps
//! are left, so the count never has to be guessed; [`SYSTEM_ADDRESS_COUNT`] is only
//! what we emit.

use crate::address::{self, AddressError};
use crate::wire::{DecodeError, Reader, Writer};
use std::fmt;
use std::net::SocketAddr;

/// `ConnectedPing`.
pub const ID_CONNECTED_PING: u8 = 0x00;
/// `ConnectedPong`.
pub const ID_CONNECTED_PONG: u8 = 0x03;
/// `ConnectionRequest`.
pub const ID_CONNECTION_REQUEST: u8 = 0x09;
/// `ConnectionRequestAccepted`.
pub const ID_CONNECTION_REQUEST_ACCEPTED: u8 = 0x10;
/// `NewIncomingConnection`.
pub const ID_NEW_INCOMING_CONNECTION: u8 = 0x13;
/// `Disconnect`.
pub const ID_DISCONNECT: u8 = 0x15;

/// Address slots we emit when we have no received count to mirror.
pub const SYSTEM_ADDRESS_COUNT: usize = 20;

/// Bytes of trailing timestamps after the address slots.
const TRAILING_TIMESTAMPS: usize = 16;

/// Why a connected packet did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnlineError {
    /// The buffer ended early.
    Truncated(DecodeError),
    /// The first byte was not the expected packet id.
    UnexpectedId {
        /// The id required.
        expected: u8,
        /// The id found.
        found: u8,
    },
    /// An embedded address did not decode.
    Address(AddressError),
}

impl From<DecodeError> for OnlineError {
    fn from(e: DecodeError) -> Self {
        Self::Truncated(e)
    }
}

impl From<AddressError> for OnlineError {
    fn from(e: AddressError) -> Self {
        Self::Address(e)
    }
}

impl fmt::Display for OnlineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(e) => write!(f, "{e}"),
            Self::UnexpectedId { expected, found } => {
                write!(f, "expected packet id {expected:#04x}, found {found:#04x}")
            }
            Self::Address(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for OnlineError {}

fn expect(r: &mut Reader<'_>, id: u8) -> Result<(), OnlineError> {
    let found = r.u8()?;
    if found == id {
        Ok(())
    } else {
        Err(OnlineError::UnexpectedId {
            expected: id,
            found,
        })
    }
}

/// Builds `ConnectionRequest`.
pub fn encode_connection_request(client_guid: i64, time: i64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(ID_CONNECTION_REQUEST).i64(client_guid).i64(time).u8(0);
    w.finish()
}

/// A decoded `ConnectionRequestAccepted`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionRequestAccepted {
    /// Our address, as the server sees it.
    pub client_addr: SocketAddr,
    /// The server's index for this connection.
    pub system_index: u16,
    /// Address slots, mostly the unassigned placeholder.
    pub system_addresses: Vec<SocketAddr>,
    /// The timestamp we sent in `ConnectionRequest`.
    pub request_time: i64,
    /// The server's own timestamp.
    pub accepted_time: i64,
}

/// Decodes `ConnectionRequestAccepted`.
pub fn decode_connection_request_accepted(
    buf: &[u8],
) -> Result<ConnectionRequestAccepted, OnlineError> {
    let mut r = Reader::new(buf);
    expect(&mut r, ID_CONNECTION_REQUEST_ACCEPTED)?;

    let client_addr = address::read(&mut r)?;
    let system_index = r.u16()?;

    let mut system_addresses = Vec::new();
    while r.remaining() > TRAILING_TIMESTAMPS {
        system_addresses.push(address::read(&mut r)?);
    }

    Ok(ConnectionRequestAccepted {
        client_addr,
        system_index,
        system_addresses,
        request_time: r.i64()?,
        accepted_time: r.i64()?,
    })
}

/// Builds `NewIncomingConnection`.
///
/// `slots` should mirror how many address slots the server sent, since that is the
/// count it is prepared to read back.
pub fn encode_new_incoming_connection(
    server: SocketAddr,
    slots: usize,
    request_time: i64,
    accepted_time: i64,
) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(ID_NEW_INCOMING_CONNECTION);
    address::write(&mut w, server);
    for _ in 0..slots {
        address::write(&mut w, address::unassigned());
    }
    w.i64(request_time).i64(accepted_time);
    w.finish()
}

/// Builds `ConnectedPing`.
pub fn encode_connected_ping(time: i64) -> Vec<u8> {
    let mut w = Writer::new();
    w.u8(ID_CONNECTED_PING).i64(time);
    w.finish()
}

/// A decoded `ConnectedPong`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectedPong {
    /// The timestamp from our ping.
    pub ping_time: i64,
    /// The server's timestamp.
    pub pong_time: i64,
}

/// Decodes `ConnectedPong`.
pub fn decode_connected_pong(buf: &[u8]) -> Result<ConnectedPong, OnlineError> {
    let mut r = Reader::new(buf);
    expect(&mut r, ID_CONNECTED_PONG)?;
    Ok(ConnectedPong {
        ping_time: r.i64()?,
        pong_time: r.i64()?,
    })
}

/// Builds `Disconnect`.
pub fn encode_disconnect() -> Vec<u8> {
    vec![ID_DISCONNECT]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn accepted(slots: usize) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(ID_CONNECTION_REQUEST_ACCEPTED);
        address::write(&mut w, "203.0.113.5:41234".parse().unwrap());
        w.u16(1);
        for _ in 0..slots {
            address::write(&mut w, address::unassigned());
        }
        w.i64(111).i64(222);
        w.finish()
    }

    /// Ten slots or twenty, both decode — the count is inferred, never assumed.
    #[test]
    fn address_slot_count_is_inferred() {
        for slots in [10, 20] {
            let decoded = decode_connection_request_accepted(&accepted(slots)).unwrap();
            assert_eq!(decoded.system_addresses.len(), slots);
            assert_eq!(decoded.request_time, 111);
            assert_eq!(decoded.accepted_time, 222);
            assert_eq!(decoded.client_addr.port(), 41234);
        }
    }

    #[test]
    fn connection_request_has_a_fixed_shape() {
        let buf = encode_connection_request(7, 9);
        assert_eq!(buf.len(), 1 + 8 + 8 + 1);
        assert_eq!(buf[0], ID_CONNECTION_REQUEST);
    }

    #[test]
    fn new_incoming_connection_slots_are_placeholders() {
        let buf = encode_new_incoming_connection("1.2.3.4:19132".parse().unwrap(), 20, 1, 2);
        assert_eq!(buf.len(), 1 + address::IPV4_LEN * 21 + 16);
    }

    #[test]
    fn connected_pong_round_trips() {
        let mut w = Writer::new();
        w.u8(ID_CONNECTED_PONG).i64(5).i64(6);
        let pong = decode_connected_pong(&w.finish()).unwrap();
        assert_eq!(pong.ping_time, 5);
        assert_eq!(pong.pong_time, 6);
    }

    #[test]
    fn wrong_id_is_reported() {
        let mut buf = accepted(20);
        buf[0] = ID_DISCONNECT;
        assert!(matches!(
            decode_connection_request_accepted(&buf),
            Err(OnlineError::UnexpectedId { .. })
        ));
    }

    #[test]
    fn truncated_prefixes_never_panic() {
        let full = accepted(20);
        for n in 0..full.len() {
            let _ = decode_connection_request_accepted(&full[..n]);
        }
    }
}
