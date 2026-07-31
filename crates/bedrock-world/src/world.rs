//! The set of columns that exist right now.
//!
//! A column is generated the first time someone asks for it and kept afterwards, so
//! that a player walking back over ground they already crossed sees the same world —
//! including whatever was built on it. In M0 nothing is built and nothing is saved
//! (ADR-011), but the identity has to hold from the start: a world that regenerates on
//! every look is a world where nothing can be changed.

use crate::chunk::Chunk;
use crate::generator::Flat;
use bedrock_blocks::Block;
use std::collections::HashMap;

/// Every column that has been asked for, and the generator that makes new ones.
#[derive(Debug)]
pub struct World {
    generator: Flat,
    chunks: HashMap<(i32, i32), Chunk>,
}

impl World {
    /// A flat world with its surface at `surface_height`.
    pub fn flat(surface_height: i32) -> Self {
        Self {
            generator: Flat::new(surface_height),
            chunks: HashMap::new(),
        }
    }

    /// Where the ground stops.
    pub fn surface_height(&self) -> i32 {
        self.generator.surface_height()
    }

    /// The column at `(x, z)`, generating it if this is the first time it is asked for.
    pub fn chunk(&mut self, x: i32, z: i32) -> &Chunk {
        let generator = &self.generator;
        self.chunks
            .entry((x, z))
            .or_insert_with(|| generator.generate(x, z))
    }

    /// The column at `(x, z)` if it already exists, without generating one.
    pub fn loaded_chunk(&self, x: i32, z: i32) -> Option<&Chunk> {
        self.chunks.get(&(x, z))
    }

    /// How many columns are held.
    pub fn loaded(&self) -> usize {
        self.chunks.len()
    }

    /// Forgets a column. It comes back generated if asked for again.
    pub fn unload(&mut self, x: i32, z: i32) -> bool {
        self.chunks.remove(&(x, z)).is_some()
    }

    /// The block at a world position, generating the column if needed.
    pub fn block_at(&mut self, x: i32, y: i32, z: i32) -> Option<&Block> {
        let (chunk_x, chunk_z) = chunk_of(x, z);
        let (local_x, local_z) = local_of(x, z);
        self.chunk(chunk_x, chunk_z).block_at(local_x, y, local_z)
    }

    /// Replaces one block, generating the column if needed. Returns whether the
    /// position was inside the world.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, block: Block) -> bool {
        let (chunk_x, chunk_z) = chunk_of(x, z);
        let (local_x, local_z) = local_of(x, z);
        let generator = &self.generator;
        self.chunks
            .entry((chunk_x, chunk_z))
            .or_insert_with(|| generator.generate(chunk_x, chunk_z))
            .set_block(local_x, y, local_z, block)
    }
}

/// Which column a world position falls in.
pub fn chunk_of(x: i32, z: i32) -> (i32, i32) {
    (
        x.div_euclid(crate::section::SECTION_SIZE as i32),
        z.div_euclid(crate::section::SECTION_SIZE as i32),
    )
}

/// Where inside its column a world position sits.
pub fn local_of(x: i32, z: i32) -> (usize, usize) {
    (
        x.rem_euclid(crate::section::SECTION_SIZE as i32) as usize,
        z.rem_euclid(crate::section::SECTION_SIZE as i32) as usize,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_blocks::{AIR, STONE};

    #[test]
    fn a_column_is_generated_once_and_then_kept() {
        let mut world = World::flat(80);
        assert_eq!(world.loaded(), 0);
        assert_eq!(world.loaded_chunk(0, 0), None);

        world.chunk(0, 0);
        world.chunk(0, 0);
        assert_eq!(world.loaded(), 1);
        assert!(world.loaded_chunk(0, 0).is_some());
    }

    /// The reason columns are kept at all: a change has to survive looking away.
    #[test]
    fn a_change_survives_asking_for_the_column_again() {
        let mut world = World::flat(80);
        assert!(world.set_block(20, 80, -3, STONE));

        assert_eq!(world.block_at(20, 80, -3), Some(&STONE));
        assert_eq!(world.block_at(21, 80, -3), Some(&AIR), "and nothing else");
    }

    #[test]
    fn unloading_a_column_puts_the_generated_world_back() {
        let mut world = World::flat(80);
        world.set_block(0, 80, 0, STONE);

        assert!(world.unload(0, 0));
        assert!(!world.unload(0, 0), "it is gone, not gone twice");

        // Asking again generates it, which is what makes the block placed above vanish.
        assert_eq!(world.block_at(0, 80, 0), Some(&AIR));
        assert_eq!(world.loaded(), 1);
    }

    /// Negative coordinates are where a `/16` and a `%` quietly disagree with the
    /// world: block -1 belongs to column -1, at local 15.
    #[test]
    fn negative_positions_belong_to_the_column_below() {
        assert_eq!(chunk_of(-1, -16), (-1, -1));
        assert_eq!(local_of(-1, -16), (15, 0));
        assert_eq!(chunk_of(16, 31), (1, 1));
        assert_eq!(local_of(16, 31), (0, 15));

        let mut world = World::flat(80);
        world.set_block(-1, 80, -16, STONE);
        assert_eq!(
            world.loaded_chunk(-1, -1).unwrap().block_at(15, 80, 0),
            Some(&STONE)
        );
    }

    #[test]
    fn the_generated_world_is_the_flat_one() {
        let mut world = World::flat(80);
        assert_eq!(world.surface_height(), 80);
        assert_eq!(world.block_at(100, 79, -100), Some(&STONE));
        assert_eq!(world.block_at(100, 80, -100), Some(&AIR));
    }
}
