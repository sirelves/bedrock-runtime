//! Where columns come from when nothing has stored one.
//!
//! M0 has one generator and it is flat: stone up to a surface, air above it. There is
//! no disk in M0 (ADR-011), so this is the only source of world there is.

use crate::chunk::{Chunk, MAX_SECTION, MIN_SECTION};
use crate::section::{SECTION_SIZE, Section};
use bedrock_blocks::{AIR, STONE};
use std::sync::Arc;

/// A flat world: solid below `surface_height`, air at it and above.
///
/// The surface is the first air block, so a player standing at `surface_height` is
/// standing on the ground rather than inside it.
///
/// Every column is the same column, so the sections are built once here and each
/// generated chunk is a handful of pointer copies. That is not an optimization for its
/// own sake: streaming a view distance means generating dozens of columns in one pass,
/// and generating identical cubes dozens of times is work with no output.
#[derive(Debug, Clone)]
pub struct Flat {
    surface_height: i32,
    template: Vec<Arc<Section>>,
}

impl Flat {
    /// A flat world with its surface at `surface_height`.
    pub fn new(surface_height: i32) -> Self {
        let stone = Arc::new(Section::uniform(STONE));
        let air = Arc::new(Section::uniform(AIR));

        let mut template = Vec::new();
        for index in MIN_SECTION..=MAX_SECTION {
            let bottom = index * SECTION_SIZE as i32;
            let top = bottom + SECTION_SIZE as i32;

            let section = if top <= surface_height {
                Arc::clone(&stone)
            } else if bottom >= surface_height {
                Arc::clone(&air)
            } else {
                Arc::new(cut_section(surface_height - bottom))
            };
            template.push(section);
        }

        Self {
            surface_height,
            template,
        }
    }

    /// Where the ground stops.
    pub fn surface_height(&self) -> i32 {
        self.surface_height
    }

    /// The column at `(x, z)`.
    pub fn generate(&self, x: i32, z: i32) -> Chunk {
        // The template is exactly `SECTION_COUNT` long by construction, so this cannot
        // be refused; falling back to an empty column keeps the promise anyway rather
        // than unwrapping in a crate that denies it.
        Chunk::from_sections(x, z, self.template.clone()).unwrap_or_else(|| Chunk::empty(x, z))
    }
}

/// A section the surface runs through: stone below `height`, air from there up.
fn cut_section(height: i32) -> Section {
    let height = height.clamp(0, SECTION_SIZE as i32) as usize;
    Section::from_fn(|_, y, _| if y < height { STONE } else { AIR })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{MIN_Y, section_index};

    #[test]
    fn the_surface_is_the_first_air_block() {
        let flat = Flat::new(80);
        let chunk = flat.generate(0, 0);

        assert_eq!(chunk.block_at(0, 79, 0), Some(&STONE), "ground");
        assert_eq!(chunk.block_at(0, 80, 0), Some(&AIR), "where the feet go");
        assert_eq!(chunk.block_at(0, 81, 0), Some(&AIR));
        assert_eq!(
            chunk.block_at(0, MIN_Y, 0),
            Some(&STONE),
            "solid to the floor"
        );
    }

    /// A surface that lands inside a section, not on its boundary, is the case a
    /// section-granular generator gets wrong.
    #[test]
    fn a_surface_inside_a_section_cuts_it() {
        let flat = Flat::new(70);
        let chunk = flat.generate(0, 0);

        assert_eq!(chunk.block_at(9, 69, 4), Some(&STONE));
        assert_eq!(chunk.block_at(9, 70, 4), Some(&AIR));
        assert_eq!(chunk.highest_used_section(), Some(section_index(69)));
    }

    #[test]
    fn a_surface_on_a_boundary_needs_no_cut_section() {
        let flat = Flat::new(64);
        let chunk = flat.generate(0, 0);

        assert_eq!(chunk.block_at(0, 63, 0), Some(&STONE));
        assert_eq!(chunk.block_at(0, 64, 0), Some(&AIR));
        assert_eq!(chunk.highest_used_section(), Some(3));

        let cut = chunk.section(3).unwrap();
        assert_eq!(
            cut.uniform_block(),
            Some(&STONE),
            "no partial section needed"
        );
    }

    /// Two columns of a flat world are the same cubes, not copies of them. Losing this
    /// turns a view distance of 8 into 289 columns' worth of allocation per player.
    #[test]
    fn columns_share_their_sections() {
        let flat = Flat::new(80);
        let one = flat.generate(0, 0);
        let far = flat.generate(1000, -1000);

        for (a, b) in one.sections().iter().zip(far.sections()) {
            assert!(Arc::ptr_eq(a, b));
        }
        assert_eq!(far.position(), (1000, -1000));
    }

    #[test]
    fn a_world_with_no_ground_is_all_air() {
        let flat = Flat::new(MIN_Y);
        let chunk = flat.generate(0, 0);
        assert_eq!(chunk.highest_used_section(), None);
        assert_eq!(chunk.block_at(0, MIN_Y, 0), Some(&AIR));
    }

    /// The whole column, not just the two blocks around the surface: an off-by-one in
    /// the section maths shows up far from where it was written.
    #[test]
    fn the_ground_is_solid_all_the_way_down_and_the_sky_all_the_way_up() {
        let flat = Flat::new(80);
        let chunk = flat.generate(0, 0);
        for y in MIN_Y..crate::chunk::MAX_Y {
            let expected = if y < 80 { &STONE } else { &AIR };
            assert_eq!(chunk.block_at(7, y, 7), Some(expected), "y={y}");
        }
    }
}
