//! Chunks, subchunks and world generation.
//!
//! **Boundary:** knows nothing about the network. Serializing a chunk for a client is
//! `bedrock-server`'s job, using `bedrock-protocol` — see `docs/ARCHITECTURE.md`.
//!
//! **Chunk sections are immutable and shared (`Arc`), mutated copy-on-write.** This is
//! not an optimization detail; it is what makes it possible to hand a section to a
//! worker thread for serialization and compression without handing out a reference to
//! live world state — see ADR-010 in `docs/DECISIONS.md`. Building mutable sections
//! first and retrofitting this later would be a rewrite.
//!
//! Persistence is not part of M0: the M0 world is generated in memory and never
//! touches disk. Storage lands in M1, in a format of our own — compatibility with
//! vanilla worlds is out of scope, see `docs/COMPATIBILITY.md`.
//!
//! Status: not started.
