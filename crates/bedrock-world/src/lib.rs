//! Chunks, subchunks, block states, block palette and world storage.
//!
//! **Boundary:** knows nothing about the network. Serializing a chunk for a client
//! is `bedrock-server`'s job, using `bedrock-protocol` — see `docs/ARCHITECTURE.md`.
//!
//! Whether the on-disk format is compatible with vanilla worlds is an open decision,
//! to be settled in M2 with its own ADR. Until then, no world produced by this
//! server should be considered migratable — see `docs/COMPATIBILITY.md`.
//!
//! Status: not started.
