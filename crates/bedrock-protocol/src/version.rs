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
//! Corroborated, **not** authoritative. A third-party server advertises whatever its
//! operator configured; agreement between two of them is strong evidence about what
//! current clients speak, not proof. Two things would upgrade it, and both are cheap
//! once they are possible:
//!
//! - probing an official Bedrock Dedicated Server we run ourselves, which advertises
//!   what Mojang shipped;
//! - completing a real login (M0.3) — a wrong protocol version fails there loudly.
//!
//! Until one of those happens, a mismatch against a real client is evidence against
//! *these constants* first, and against the code reading them second.

/// Numeric protocol version, sent in `Login` and in the offline pong.
///
/// See the module docs for provenance and how far to trust it.
pub const PROTOCOL_VERSION: u32 = 1001;

/// Human-readable Minecraft version this protocol belongs to.
///
/// See the module docs for provenance and how far to trust it.
pub const MINECRAFT_VERSION: &str = "1.26.30";
