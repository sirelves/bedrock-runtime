//! Bedrock game packet definitions and codec.
//!
//! **Boundary:** pure. This crate turns bytes into packet types and back. It holds
//! no game state, performs no I/O, and does not know what a player or a world is —
//! see `docs/ARCHITECTURE.md`.
//!
//! A single protocol version is supported at a time; there is no translation layer
//! and no version abstraction — see ADR-004 in `docs/DECISIONS.md`.
//!
//! **Security:** this is the second thing an unauthenticated attacker reaches. No
//! panics on the decode path, no allocation sized by an attacker-controlled field.
//! A malformed packet ends that session and nothing else.
//!
//! Status: not started. Milestone M0.4 — see `docs/ROADMAP.md`.

pub mod version;
