//! Session cryptography: key agreement, key derivation and the stream cipher.
//!
//! **Primitives only.** The login chain and batch compression used to live here and
//! were moved to `bedrock-protocol`: JWT validation knows Minecraft (XUID, skin data,
//! the shape of `Login`) and compression is not cryptography. Keeping them here made
//! the one crate that must be auditable in isolation the one that mixed the most
//! layers — see ADR-009 in `docs/DECISIONS.md`.
//!
//! What remains is small on purpose: ECDH over P-384, derivation of the session key
//! from the shared secret and salt, and the symmetric cipher over the packet stream.
//!
//! Non-negotiable: no key material is ever logged, at any level; comparisons of secret
//! material are constant-time. See `SECURITY.md`.
//!
//! Status: not started. Milestone M0.3 — see `docs/ROADMAP.md`.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod agreement;
pub mod jwt;
