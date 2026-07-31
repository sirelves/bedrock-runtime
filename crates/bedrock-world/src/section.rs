//! A 16×16×16 cube of blocks, immutable once built.
//!
//! **Immutability is the point** (ADR-010): a section can be handed to another thread
//! for serialization and compression because nothing can change it underneath. Writing
//! a block produces a new section; the chunk swaps the pointer.
//!
//! Blocks are stored as a palette plus one index per position. A section that is all
//! one block — most of a flat world, and most of any world's sky — carries no index
//! array at all.

use bedrock_blocks::Block;

/// Blocks along each edge of a section.
pub const SECTION_SIZE: usize = 16;

/// Blocks in a whole section.
pub const BLOCKS_PER_SECTION: usize = SECTION_SIZE * SECTION_SIZE * SECTION_SIZE;

/// A cube of blocks.
///
/// Cloning is not free — it copies the index array when there is one — which is why the
/// chunk holds sections behind an `Arc` and clones only to write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// The distinct blocks in this section. Never empty.
    palette: Vec<Block>,
    /// One palette index per position, or `None` when the section is a single block
    /// everywhere.
    indices: Option<Box<[u16; BLOCKS_PER_SECTION]>>,
}

/// Where a position sits in the index array.
///
/// The order is internal and nothing outside this module may assume it: a caller that
/// needs blocks in a particular order asks for them by coordinate. Making the wire
/// format's order the storage order would be exactly the coupling ADR-008 removes.
fn offset(x: usize, y: usize, z: usize) -> Option<usize> {
    if x >= SECTION_SIZE || y >= SECTION_SIZE || z >= SECTION_SIZE {
        return None;
    }
    Some((y * SECTION_SIZE + z) * SECTION_SIZE + x)
}

impl Section {
    /// A section that is `block` everywhere.
    pub fn uniform(block: Block) -> Self {
        Self {
            palette: vec![block],
            indices: None,
        }
    }

    /// A section built by asking for the block at every position.
    ///
    /// Builds the palette and the indices in one pass. The alternative — starting
    /// uniform and writing four thousand times — copies the index array once per
    /// block, which is how generating a section becomes the most expensive thing a
    /// generator does.
    pub fn from_fn(mut block_at: impl FnMut(usize, usize, usize) -> Block) -> Self {
        let mut palette: Vec<Block> = Vec::new();
        let mut indices = Box::new([0u16; BLOCKS_PER_SECTION]);

        for x in 0..SECTION_SIZE {
            for y in 0..SECTION_SIZE {
                for z in 0..SECTION_SIZE {
                    let block = block_at(x, y, z);
                    let index = match palette.iter().position(|entry| *entry == block) {
                        Some(index) => index,
                        None => {
                            palette.push(block);
                            palette.len() - 1
                        }
                    };
                    if let (Some(offset), Ok(index)) = (offset(x, y, z), u16::try_from(index)) {
                        indices[offset] = index;
                    }
                }
            }
        }

        // One block everywhere is the single-value case, and carrying an index array
        // that says so would cost four kilobytes to encode nothing.
        match palette.len() {
            1 => Self {
                palette,
                indices: None,
            },
            _ => Self {
                palette,
                indices: Some(indices),
            },
        }
    }

    /// The block at a position inside the section, or `None` if the position is outside
    /// it.
    pub fn get(&self, x: usize, y: usize, z: usize) -> Option<&Block> {
        let offset = offset(x, y, z)?;
        let index = match &self.indices {
            None => 0,
            Some(indices) => usize::from(*indices.get(offset)?),
        };
        self.palette.get(index)
    }

    /// Which palette entry a position uses, or `None` if the position is outside the
    /// section.
    ///
    /// A serializer needs this: the wire format stores indices into a palette too, so
    /// walking positions and searching the palette for each block would be looking up
    /// what is already known.
    pub fn index_at(&self, x: usize, y: usize, z: usize) -> Option<u16> {
        let offset = offset(x, y, z)?;
        match &self.indices {
            None => Some(0),
            Some(indices) => indices.get(offset).copied(),
        }
    }

    /// The one block this section is made of, when it is made of one.
    ///
    /// The single-value case is worth naming: it is what lets a serializer write a
    /// whole section as a header and one id instead of walking four thousand
    /// positions.
    pub fn uniform_block(&self) -> Option<&Block> {
        match self.indices {
            None => self.palette.first(),
            Some(_) => None,
        }
    }

    /// The distinct blocks in this section.
    pub fn palette(&self) -> &[Block] {
        &self.palette
    }

    /// Whether this section holds nothing but air.
    ///
    /// Judged from the palette, so a block that was placed and then removed keeps the
    /// section looking non-empty until it is rebuilt. That errs towards sending a
    /// section that did not need sending, which costs bytes; the other direction would
    /// drop blocks a player can see.
    pub fn is_empty(&self) -> bool {
        self.palette.iter().all(Block::is_air)
    }

    /// A copy of this section with one block replaced, or `None` if the position is
    /// outside it.
    ///
    /// Copy-on-write: the section this is called on is untouched, and any thread
    /// holding it keeps looking at the world as it was.
    pub fn with_block(&self, x: usize, y: usize, z: usize, block: Block) -> Option<Self> {
        let offset = offset(x, y, z)?;

        if self.get(x, y, z) == Some(&block) {
            return Some(self.clone());
        }

        let mut palette = self.palette.clone();
        let index = match palette.iter().position(|entry| *entry == block) {
            Some(index) => index,
            None => {
                palette.push(block);
                palette.len() - 1
            }
        };
        let index = u16::try_from(index).ok()?;

        let mut indices = match &self.indices {
            Some(indices) => indices.clone(),
            // Was uniform: every position points at palette entry zero, which is what
            // it was made of.
            None => Box::new([0u16; BLOCKS_PER_SECTION]),
        };
        *indices.get_mut(offset)? = index;

        Some(Self {
            palette,
            indices: Some(indices),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_blocks::{AIR, STONE};

    #[test]
    fn a_uniform_section_is_that_block_everywhere() {
        let section = Section::uniform(STONE);
        assert_eq!(section.uniform_block(), Some(&STONE));
        for (x, y, z) in [(0, 0, 0), (15, 15, 15), (3, 9, 12)] {
            assert_eq!(section.get(x, y, z), Some(&STONE), "at {x},{y},{z}");
        }
    }

    #[test]
    fn a_position_outside_the_cube_is_none_not_a_panic() {
        let section = Section::uniform(STONE);
        assert_eq!(section.get(16, 0, 0), None);
        assert_eq!(section.get(0, 99, 0), None);
        assert!(section.with_block(0, 0, 16, AIR).is_none());
    }

    /// The property ADR-010 rests on: writing does not mutate what anyone else is
    /// holding.
    #[test]
    fn writing_leaves_the_original_alone() {
        let before = Section::uniform(STONE);
        let after = before.with_block(1, 2, 3, AIR).unwrap();

        assert_eq!(before.get(1, 2, 3), Some(&STONE), "the original is frozen");
        assert_eq!(after.get(1, 2, 3), Some(&AIR));
        assert_eq!(after.get(0, 0, 0), Some(&STONE), "and only one block moved");
    }

    /// Every position must map to its own slot. A wrong offset formula shows up as one
    /// write landing on top of another, which this catches and a spot check does not.
    #[test]
    fn every_position_is_its_own_slot() {
        let mut seen = std::collections::HashSet::new();
        for x in 0..SECTION_SIZE {
            for y in 0..SECTION_SIZE {
                for z in 0..SECTION_SIZE {
                    assert!(seen.insert(offset(x, y, z).unwrap()), "{x},{y},{z} repeats");
                }
            }
        }
        assert_eq!(seen.len(), BLOCKS_PER_SECTION);
    }

    #[test]
    fn writing_the_same_block_back_changes_nothing() {
        let section = Section::uniform(STONE);
        let again = section.with_block(4, 4, 4, STONE).unwrap();
        assert_eq!(again.palette(), section.palette());
        assert_eq!(again.get(4, 4, 4), Some(&STONE));
    }

    #[test]
    fn the_palette_grows_only_for_new_blocks() {
        let section = Section::uniform(STONE);
        let one = section.with_block(0, 0, 0, AIR).unwrap();
        assert_eq!(one.palette().len(), 2);

        let two = one.with_block(1, 0, 0, AIR).unwrap();
        assert_eq!(two.palette().len(), 2, "air was already in the palette");
        assert_eq!(two.get(1, 0, 0), Some(&AIR));
    }

    #[test]
    fn building_by_position_keeps_every_block_where_it_was_put() {
        let half = Section::from_fn(|_, y, _| if y < 8 { STONE } else { AIR });
        assert_eq!(half.palette().len(), 2);
        assert_eq!(half.get(0, 7, 0), Some(&STONE));
        assert_eq!(half.get(15, 8, 15), Some(&AIR));
        assert_eq!(half.uniform_block(), None);
    }

    /// One block everywhere must come out as the single-value case whichever way it was
    /// built, or a flat world pays four kilobytes a section to say nothing.
    #[test]
    fn building_one_block_everywhere_stays_uniform() {
        let section = Section::from_fn(|_, _, _| STONE);
        assert_eq!(section.uniform_block(), Some(&STONE));
        assert_eq!(section, Section::uniform(STONE));
    }

    #[test]
    fn a_section_of_air_is_empty_and_one_of_stone_is_not() {
        assert!(Section::uniform(AIR).is_empty());
        assert!(!Section::uniform(STONE).is_empty());

        let mixed = Section::uniform(AIR).with_block(0, 0, 0, STONE).unwrap();
        assert!(!mixed.is_empty());
    }
}
