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
//! ```
//! use bedrock_world::World;
//! use bedrock_blocks::STONE;
//!
//! let mut world = World::flat(80);
//! assert_eq!(world.block_at(0, 79, 0), Some(&STONE), "standing on the ground");
//! ```

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod chunk;
pub mod generator;
pub mod section;
pub mod world;

pub use chunk::Chunk;
pub use generator::Flat;
pub use section::Section;
pub use world::World;
