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
//! Status: not started. This is milestone M0.2 and the highest-risk item in the
//! project — see `docs/ROADMAP.md`.

/// Default UDP port for Bedrock over IPv4.
pub const DEFAULT_PORT_V4: u16 = 19132;

/// Default UDP port for Bedrock over IPv6.
pub const DEFAULT_PORT_V6: u16 = 19133;

/// The 16-byte constant carried by every offline (pre-connection) RakNet packet.
pub const MAGIC: [u8; 16] = [
    0x00, 0xff, 0xff, 0x00, 0xfe, 0xfe, 0xfe, 0xfe, 0xfd, 0xfd, 0xfd, 0xfd, 0x12, 0x34, 0x56, 0x78,
];
