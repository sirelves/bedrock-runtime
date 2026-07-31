//! A column of the world: every section at one horizontal position, stacked.
//!
//! The overworld runs from y = -64 to y = 320, which is 24 sections. That shape is the
//! world's, not the wire's — `bedrock-protocol` describes the same span for its own
//! reasons and the two are checked against each other in `bedrock-server`.

use crate::section::{SECTION_SIZE, Section};
use bedrock_blocks::{AIR, Block};
use std::sync::Arc;

/// The lowest section index in the overworld: y = -64.
pub const MIN_SECTION: i32 = -4;

/// The highest section index in the overworld: the one containing y = 319.
pub const MAX_SECTION: i32 = 19;

/// How many sections a full column has.
pub const SECTION_COUNT: usize = (MAX_SECTION - MIN_SECTION + 1) as usize;

/// The lowest block a column can hold.
pub const MIN_Y: i32 = MIN_SECTION * SECTION_SIZE as i32;

/// One above the highest block a column can hold.
pub const MAX_Y: i32 = (MAX_SECTION + 1) * SECTION_SIZE as i32;

/// A column of sections at a fixed `(x, z)` in chunk coordinates.
///
/// Sections are behind `Arc` so that handing one to a serializer costs a pointer copy
/// and hands out no way to change it — see ADR-010.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    x: i32,
    z: i32,
    sections: Vec<Arc<Section>>,
}

impl Chunk {
    /// A column of air at `(x, z)`.
    ///
    /// Every section starts out sharing one allocation: an empty world costs one cube,
    /// not twenty-four per column.
    pub fn empty(x: i32, z: i32) -> Self {
        let air = Arc::new(Section::uniform(AIR));
        Self {
            x,
            z,
            sections: vec![air; SECTION_COUNT],
        }
    }

    /// A column built from its sections, bottom first.
    ///
    /// Fewer sections than [`SECTION_COUNT`] are padded with air; more are refused,
    /// because a column taller than the dimension is a caller bug and silently cutting
    /// it would hide it.
    pub fn from_sections(x: i32, z: i32, sections: Vec<Arc<Section>>) -> Option<Self> {
        if sections.len() > SECTION_COUNT {
            return None;
        }
        let mut sections = sections;
        let air = Arc::new(Section::uniform(AIR));
        sections.resize(SECTION_COUNT, air);
        Some(Self { x, z, sections })
    }

    /// Where this column sits, in chunk coordinates.
    pub fn position(&self) -> (i32, i32) {
        (self.x, self.z)
    }

    /// The sections, bottom first. The first one is [`MIN_SECTION`].
    pub fn sections(&self) -> &[Arc<Section>] {
        &self.sections
    }

    /// One section by its index, where the index is a section-sized slice of world y.
    pub fn section(&self, index: i32) -> Option<&Arc<Section>> {
        let slot = usize::try_from(index.checked_sub(MIN_SECTION)?).ok()?;
        self.sections.get(slot)
    }

    /// The highest section that is not entirely air, if any.
    ///
    /// This is how much of the column is worth writing: everything above is sky, and
    /// the client fills sky in for itself.
    pub fn highest_used_section(&self) -> Option<i32> {
        self.sections
            .iter()
            .rposition(|section| !section.is_empty())
            .and_then(|slot| i32::try_from(slot).ok())
            .map(|slot| MIN_SECTION + slot)
    }

    /// The block at a position, with `y` in world coordinates and `x`/`z` relative to
    /// the column.
    pub fn block_at(&self, x: usize, y: i32, z: usize) -> Option<&Block> {
        let section = self.section(section_index(y))?;
        section.get(x, section_offset(y), z)
    }

    /// Replaces one block, copying the section it lands in.
    ///
    /// Returns whether the position was inside the column. Sections other than the one
    /// written keep sharing whatever they shared before.
    pub fn set_block(&mut self, x: usize, y: i32, z: usize, block: Block) -> bool {
        let index = section_index(y);
        let Some(slot) = index
            .checked_sub(MIN_SECTION)
            .and_then(|slot| usize::try_from(slot).ok())
        else {
            return false;
        };
        let Some(section) = self.sections.get(slot) else {
            return false;
        };
        let Some(updated) = section.with_block(x, section_offset(y), z, block) else {
            return false;
        };
        self.sections[slot] = Arc::new(updated);
        true
    }
}

/// Which section a world y falls in. Negative y rounds down, not towards zero.
pub fn section_index(y: i32) -> i32 {
    y.div_euclid(SECTION_SIZE as i32)
}

/// How far up inside its section a world y sits.
pub fn section_offset(y: i32) -> usize {
    y.rem_euclid(SECTION_SIZE as i32) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_blocks::STONE;

    #[test]
    fn the_overworld_is_twenty_four_sections_tall() {
        assert_eq!(SECTION_COUNT, 24);
        assert_eq!(MIN_Y, -64);
        assert_eq!(MAX_Y, 320);
    }

    /// Rounding towards zero would put y = -1 and y = 0 in the same section and leave
    /// the bottom of the world one section short.
    #[test]
    fn negative_heights_round_down() {
        assert_eq!(section_index(0), 0);
        assert_eq!(section_index(15), 0);
        assert_eq!(section_index(-1), -1);
        assert_eq!(section_index(-64), -4);
        assert_eq!(section_offset(-1), 15);
        assert_eq!(section_offset(-64), 0);
    }

    #[test]
    fn an_empty_column_is_air_from_floor_to_ceiling() {
        let chunk = Chunk::empty(3, -7);
        assert_eq!(chunk.position(), (3, -7));
        assert_eq!(chunk.highest_used_section(), None);
        for y in [MIN_Y, 0, 80, MAX_Y - 1] {
            assert_eq!(chunk.block_at(0, y, 0), Some(&AIR), "at y={y}");
        }
    }

    #[test]
    fn outside_the_column_reads_nothing_and_writes_nothing() {
        let mut chunk = Chunk::empty(0, 0);
        assert_eq!(chunk.block_at(0, MAX_Y, 0), None);
        assert_eq!(chunk.block_at(0, MIN_Y - 1, 0), None);
        assert!(!chunk.set_block(0, MAX_Y, 0, STONE));
        assert!(!chunk.set_block(16, 0, 0, STONE));
    }

    #[test]
    fn a_written_block_reads_back() {
        let mut chunk = Chunk::empty(0, 0);
        assert!(chunk.set_block(5, -33, 9, STONE));
        assert_eq!(chunk.block_at(5, -33, 9), Some(&STONE));
        assert_eq!(chunk.block_at(5, -32, 9), Some(&AIR), "only that one block");
        assert_eq!(chunk.highest_used_section(), Some(section_index(-33)));
    }

    /// Writing must copy one section, not the column. If the others stopped being
    /// shared, a single block placement would cost twenty-four cubes.
    #[test]
    fn writing_one_block_leaves_the_other_sections_shared() {
        let mut chunk = Chunk::empty(0, 0);
        let untouched = Arc::clone(&chunk.sections[0]);
        chunk.set_block(0, 200, 0, STONE);
        assert!(Arc::ptr_eq(&untouched, &chunk.sections[0]));
    }

    /// A snapshot taken before a write keeps showing the world as it was, which is the
    /// whole reason serialization can leave the tick.
    #[test]
    fn a_snapshot_survives_a_later_write() {
        let mut chunk = Chunk::empty(0, 0);
        let snapshot = Arc::clone(chunk.section(section_index(64)).unwrap());
        chunk.set_block(1, 64, 1, STONE);

        assert_eq!(snapshot.get(1, section_offset(64), 1), Some(&AIR));
        assert_eq!(chunk.block_at(1, 64, 1), Some(&STONE));
    }

    #[test]
    fn a_column_taller_than_the_dimension_is_refused() {
        let stone = Arc::new(Section::uniform(STONE));
        let sections = vec![stone; SECTION_COUNT + 1];
        assert!(Chunk::from_sections(0, 0, sections).is_none());
    }

    #[test]
    fn a_short_column_is_padded_with_air() {
        let stone = Arc::new(Section::uniform(STONE));
        let sections = vec![stone; 2];
        let chunk = Chunk::from_sections(0, 0, sections).unwrap();
        assert_eq!(chunk.sections().len(), SECTION_COUNT);
        assert_eq!(chunk.block_at(0, MIN_Y, 0), Some(&STONE));
        assert_eq!(chunk.block_at(0, MAX_Y - 1, 0), Some(&AIR));
        assert_eq!(chunk.highest_used_section(), Some(MIN_SECTION + 1));
    }
}
