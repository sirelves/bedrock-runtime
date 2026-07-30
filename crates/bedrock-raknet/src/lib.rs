//! RakNet transport over UDP.
//!
//! This crate is a generic reliability layer: offline discovery (ping/pong),
//! connection establishment, MTU negotiation, ordering, fragmentation and
//! reassembly, ACK/NACK and retransmission.
//!
//! **Boundary:** this crate knows nothing about Minecraft. If a game packet type
//! appears here, the design is wrong — see `docs/ARCHITECTURE.md`.
//!
//! **Security:** every buffer sized from a remote field needs an explicit upper
//! bound, and fragment reassembly needs a per-session memory cap and a timeout.
//! See `SECURITY.md`.
//!
//! Status: the offline (pre-connection) phase is being built first, because it is what
//! confirms the target protocol version without needing any of the rest — see M0.1a in
//! `docs/ROADMAP.md`. The connected phase is M0.2 and the highest-risk item in the
//! project.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod address;
pub mod advertisement;
pub mod connect;
pub mod datagram;
pub mod frame;
pub mod offline;
pub mod online;
pub mod retransmit;
pub mod split;
pub mod wire;

/// Default UDP port for Bedrock over IPv4.
pub const DEFAULT_PORT_V4: u16 = 19132;

/// Default UDP port for Bedrock over IPv6.
pub const DEFAULT_PORT_V6: u16 = 19133;

/// IPv4 (20) plus UDP (8) header bytes.
///
/// RakNet counts these in the MTU it advertises, while a datagram we build does not
/// include them. Confusing the two makes every full-size packet 28 bytes too big.
pub const UDP_IP_OVERHEAD: usize = 28;

/// Largest UDP payload RakNet probes with. Bedrock clients start here and walk down.
pub const MAX_MTU: usize = 1492;

/// The 16-byte constant carried by every offline (pre-connection) RakNet packet.
pub const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];
