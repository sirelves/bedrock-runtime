//! The target protocol version.
//!
//! This module is the single source of truth for which Bedrock version the server
//! speaks. `docs/COMPATIBILITY.md` is derived from it, not the other way around.
//!
//! The values are deliberately unset. Filling them with a plausible-looking number
//! before a real client has confirmed it would be inventing data — and a wrong
//! protocol number shows up as "the server never appears in the list", with no
//! error message. They get set in M0.1, from a captured login.

/// Numeric protocol version sent in `Login` and in the offline pong.
///
/// Unset until confirmed by capture (M0.1).
pub const PROTOCOL_VERSION: Option<u32> = None;

/// Human-readable Minecraft version string, e.g. `"1.21.0"`.
///
/// Unset until confirmed by capture (M0.1).
pub const MINECRAFT_VERSION: Option<&str> = None;
