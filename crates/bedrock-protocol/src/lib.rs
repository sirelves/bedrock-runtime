//! The Bedrock wire format: login chain, batching, compression and the packet codec.
//!
//! **Boundary:** pure. This crate turns bytes into packet types and back. It holds no
//! game state, performs no I/O, and does not know what a player or a world is — see
//! `docs/ARCHITECTURE.md`.
//!
//! It owns the runtime-id palette, because a runtime id is a per-version network
//! identifier. Block identity itself lives in `bedrock-blocks`, which `bedrock-world`
//! also uses — see ADR-008 in `docs/DECISIONS.md`.
//!
//! A single protocol version is supported at a time; there is no translation layer and
//! no version abstraction — see ADR-004.
//!
//! **Security:** this is the second thing an unauthenticated attacker reaches. No panics
//! on the decode path, no allocation sized by an attacker-controlled field, an explicit
//! output cap on decompression. A malformed packet ends that session and nothing else.
//!
//! Status: not started. Milestones M0.3 and M0.4 — see `docs/ROADMAP.md`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod bytes;
pub mod version;
