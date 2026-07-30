//! The shared block vocabulary: what a block *is*, independent of how it is stored
//! and how it is sent.
//!
//! This crate exists because `bedrock-world` and `bedrock-protocol` both need to name
//! blocks and neither may depend on the other — see ADR-008 in `docs/DECISIONS.md`.
//! It is a leaf crate: no internal dependencies, no I/O.
//!
//! **What lives here:** the identity of a block — its namespaced name and its state
//! properties.
//!
//! **What does not:** the runtime-id palette. A runtime id is a per-version network
//! identifier, so the name-to-runtime-id mapping belongs to `bedrock-protocol`. Storage
//! uses names and properties, never runtime ids.
//!
//! Status: not started.
