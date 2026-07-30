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
//! # How confident this is
//!
//! Corroborated by three independent sources now: two unrelated public servers, and
//! the Minecraft Wiki, which lists 1001 as the protocol of release 26.35 — the current
//! stable version. Still not ground truth from a client.
//!
//! Ground truth turns out to be cheap, and we know exactly how to get it. A client
//! states its protocol version in `RequestNetworkSettings`, **in the clear, before any
//! encryption**, as the first thing it sends after the RakNet handshake. A capture on
//! 2026-07-30 read 975 out of a real client — that client was on release 26.23, a few
//! versions behind, which is why the number differed rather than 1001 being wrong.
//!
//! See `crates/bedrock-protocol/tests/first_contact.rs`. Point a client on 26.35 at the
//! server and the constant below is confirmed or corrected in one connection, without a
//! line of crypto.

/// Numeric protocol version, sent in `Login` and in the offline pong.
///
/// See the module docs for provenance and how far to trust it.
pub const PROTOCOL_VERSION: u32 = 1001;

/// Human-readable Minecraft version this protocol belongs to.
///
/// See the module docs for provenance and how far to trust it.
pub const MINECRAFT_VERSION: &str = "1.26.30";
