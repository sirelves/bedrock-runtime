//! Socket addresses on the wire: `u8` version tag, then for IPv4 four
//! **bitwise-complemented** octets and a big-endian port.
//!
//! The complement is a RakNet quirk, confirmed on 2026-07-30 by comparing the address
//! a live server reported for us against our real public address — the complemented
//! reading matched, the raw one did not. It matters because a misread address looks
//! plausible rather than broken: the two readings are complements of each other.
//!
//! Treat any decoded remote address as a claim by a peer about itself.
//!
//! IPv6 is not decoded. Its RakNet layout is a dump of a platform `sockaddr_in6` and
//! varies with where the peer was compiled; [`AddressError::UnsupportedVersion`] says
//! so rather than guessing.

use crate::wire::{DecodeError, Reader, Writer};
use std::fmt;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// Encoded size of an IPv4 address.
pub const IPV4_LEN: usize = 7;

const VERSION_IPV4: u8 = 4;

/// Why an address did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressError {
    /// The buffer ended early.
    Truncated(DecodeError),
    /// A version tag we do not decode.
    UnsupportedVersion(u8),
}

impl From<DecodeError> for AddressError {
    fn from(e: DecodeError) -> Self {
        Self::Truncated(e)
    }
}

impl fmt::Display for AddressError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(e) => write!(f, "{e}"),
            Self::UnsupportedVersion(v) => write!(f, "address version {v} unsupported (IPv4 only)"),
        }
    }
}

impl std::error::Error for AddressError {}

/// RakNet's own placeholder for an unused address slot: `255.255.255.255:0`, which is
/// seven zero bytes on the wire.
pub fn unassigned() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, 0))
}

/// Whether an address slot is empty under either convention in use.
///
/// RakNet writes `255.255.255.255:0`; a live PocketMine server was observed filling all
/// twenty slots with `0.0.0.0:0` instead. Both mean the same thing, and a peer that
/// recognised only one would misread the other as a routable address.
pub fn is_empty_slot(addr: SocketAddr) -> bool {
    addr.port() == 0
        && match addr.ip() {
            std::net::IpAddr::V4(ip) => ip.is_unspecified() || ip.is_broadcast(),
            std::net::IpAddr::V6(ip) => ip.is_unspecified(),
        }
}

/// Writes an address. An IPv6 input is written as [`unassigned`], since sending a
/// layout we cannot verify is worse than sending the placeholder peers already accept.
pub fn write(w: &mut Writer, addr: SocketAddr) -> &mut Writer {
    let v4 = match addr {
        SocketAddr::V4(v4) => v4,
        SocketAddr::V6(_) => SocketAddrV4::new(Ipv4Addr::BROADCAST, 0),
    };
    w.u8(VERSION_IPV4);
    for octet in v4.ip().octets() {
        w.u8(!octet);
    }
    w.u16(v4.port())
}

/// Reads an address.
pub fn read(r: &mut Reader<'_>) -> Result<SocketAddr, AddressError> {
    read_inner(r, true)
}

/// Reads an address without complementing the octets. For probes comparing readings
/// on a capture; the server has no reason to call this.
pub fn read_raw(r: &mut Reader<'_>) -> Result<SocketAddr, AddressError> {
    read_inner(r, false)
}

fn read_inner(r: &mut Reader<'_>, complement: bool) -> Result<SocketAddr, AddressError> {
    let version = r.u8()?;
    if version != VERSION_IPV4 {
        return Err(AddressError::UnsupportedVersion(version));
    }
    let mut octets = r.array::<4>()?;
    if complement {
        for octet in &mut octets {
            *octet = !*octet;
        }
    }
    let port = r.u16()?;
    Ok(SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::from(octets),
        port,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipv4_round_trips() {
        for s in ["192.168.0.1:19132", "8.8.8.8:53", "0.0.0.0:0"] {
            let addr: SocketAddr = s.parse().unwrap();
            let mut w = Writer::new();
            write(&mut w, addr);
            let buf = w.finish();
            assert_eq!(buf.len(), IPV4_LEN);
            assert_eq!(read(&mut Reader::new(&buf)).unwrap(), addr, "{s}");
        }
    }

    #[test]
    fn octets_go_out_complemented() {
        let mut w = Writer::new();
        write(&mut w, "192.168.0.1:1".parse().unwrap());
        assert_eq!(w.finish(), vec![4, !192, !168, !0, !1, 0x00, 0x01]);
    }

    #[test]
    fn unassigned_is_all_zeroes() {
        let mut w = Writer::new();
        write(&mut w, unassigned());
        assert_eq!(w.finish(), vec![4, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn both_empty_slot_conventions_are_recognised() {
        for s in ["255.255.255.255:0", "0.0.0.0:0"] {
            assert!(is_empty_slot(s.parse().unwrap()), "{s}");
        }
        for s in ["255.255.255.255:19132", "1.2.3.4:0", "8.8.8.8:53"] {
            assert!(!is_empty_slot(s.parse().unwrap()), "{s}");
        }
    }

    #[test]
    fn non_ipv4_version_is_reported() {
        for tag in [6u8, 99] {
            let buf = [tag, 0, 0, 0, 0, 0, 0];
            assert_eq!(
                read(&mut Reader::new(&buf)),
                Err(AddressError::UnsupportedVersion(tag))
            );
        }
    }

    #[test]
    fn truncated_address_fails_cleanly() {
        assert!(matches!(
            read(&mut Reader::new(&[4, 1, 2])),
            Err(AddressError::Truncated(_))
        ));
    }

    /// Both readings look like real addresses, and the port is untouched by either.
    #[test]
    fn the_two_readings_are_complements() {
        let buf = [4, 63, 87, 255, 254, 0x4a, 0xbc];
        let a = read(&mut Reader::new(&buf)).unwrap();
        let b = read_raw(&mut Reader::new(&buf)).unwrap();
        assert_eq!(a.ip().to_string(), "192.168.0.1");
        assert_eq!(b.ip().to_string(), "63.87.255.254");
        assert_eq!(a.port(), b.port());
    }
}
