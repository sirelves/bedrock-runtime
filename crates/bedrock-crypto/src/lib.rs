//! Login chain validation, key agreement, stream cipher and batch compression.
//!
//! Isolated in its own crate because this is the surface where a mistake becomes a
//! vulnerability, and because it should be auditable without reading the rest of
//! the server.
//!
//! Non-negotiable: the JWT signing algorithm is fixed by us, never taken from the
//! token; no key material is ever logged, at any level; comparisons of secret
//! material are constant-time. See `SECURITY.md`.
//!
//! Status: not started. Milestone M0.3 — see `docs/ROADMAP.md`.
