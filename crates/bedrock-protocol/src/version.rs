//! The target protocol version.
//!
//! This module is the single source of truth for which Bedrock version the server
//! speaks. `docs/COMPATIBILITY.md` is derived from it, not the other way around.
//!
//! # Where these numbers came from
//!
//! Four public servers were probed on 2026-07-30 with
//! `cargo run -p bedrock-raknet --example ping`. Two of them — Lifeboat and
//! NetherGames, unrelated operators running different server software — independently
//! advertised protocol `1001` for Minecraft `1.26.30`. The captures are committed
//! under `crates/bedrock-raknet/tests/fixtures/` and pinned by a test.
//!
//! The other two advertised protocol `121` and `1`, filler in front of multi-version
//! proxies. That is precisely why one source was never going to be enough: the
//! corroboration is what carries the weight here, not any single number.
//!
//! # Confirmed
//!
//! An up-to-date Minecraft client connected to our server on 2026-07-30 and declared
//! `1001` in `RequestNetworkSettings` — in the clear, before any encryption, as the
//! first thing it sent after the RakNet handshake. That is ground truth from the thing
//! we are actually serving, not inference from what other servers advertise.
//!
//! The capture is `tests/fixtures/request-network-settings-1001.bin`, and a test
//! asserts the constant below still matches it. If the target moves, that test fails
//! for a few cents instead of costing a day of debugging cryptography that was never
//! broken.
//!
//! Three earlier sources agreed on the same number — two unrelated public servers and
//! the Minecraft Wiki, which lists 1001 as the protocol of release 26.35. Agreement is
//! reassuring; the client is what settles it.
//!
//! [`MINECRAFT_VERSION`] is display-only and less certain: the public servers advertised
//! `1.26.30` while the wiki calls the release `26.35`. The client compares the number,
//! not the name, so nothing depends on resolving it.

/// Numeric protocol version, sent in `Login` and in the offline pong.
///
/// See the module docs for provenance and how far to trust it.
pub const PROTOCOL_VERSION: u32 = 1001;

/// Human-readable Minecraft version this protocol belongs to.
///
/// See the module docs for provenance and how far to trust it.
pub const MINECRAFT_VERSION: &str = "1.26.30";
