//! Turning a column of world into a `LevelChunk`.
//!
//! This is the seam the architecture puts here on purpose: `bedrock-world` stores
//! blocks and knows nothing about the wire, `bedrock-protocol` writes bytes and knows
//! nothing about the world, and neither may depend on the other. Something has to hold
//! both, and this crate is the only one allowed to.

use bedrock_protocol::batch::Packet;
use bedrock_protocol::level_chunk::{
    self, BLOCKS_PER_SUBCHUNK, RUNTIME_ID_AIR, Subchunk, block_offset,
};
use bedrock_protocol::palette;
use bedrock_world::Chunk;
use bedrock_world::chunk::MIN_SECTION;
use bedrock_world::section::{SECTION_SIZE, Section};

/// One subchunk's palette and, when it needs one, its indices.
struct Encoded {
    palette: Vec<i32>,
    indices: Option<Vec<u16>>,
}

/// How many subchunks of a column carry blocks.
///
/// Everything above the highest used section is sky, and the client draws sky without
/// being told. Sending it would be twenty-four subchunks per column where ten will do.
pub fn subchunk_count(chunk: &Chunk) -> usize {
    match chunk.highest_used_section() {
        None => 0,
        Some(top) => usize::try_from(top - MIN_SECTION + 1).unwrap_or(0),
    }
}

/// Builds the `LevelChunk` for a column.
pub fn column_packet(chunk: &Chunk, biome: i32) -> Packet {
    let (x, z) = chunk.position();
    let count = subchunk_count(chunk);

    let encoded: Vec<Encoded> = chunk
        .sections()
        .iter()
        .take(count)
        .map(|section| encode_section(section))
        .collect();

    let views: Vec<Subchunk<'_>> = encoded
        .iter()
        .map(|section| Subchunk {
            palette: &section.palette,
            indices: section.indices.as_deref(),
        })
        .collect();

    level_chunk::level_chunk(x, z, count, &level_chunk::column_payload(&views, biome))
}

/// Maps one section's blocks onto this version's runtime ids.
///
/// A block whose runtime id is unknown is written as air. The alternative — picking a
/// neighbouring id — puts a block in the world that nobody placed, and a hole is easier
/// to recognise as a missing entry in [`palette`] than a stray block is.
fn encode_section(section: &Section) -> Encoded {
    let palette: Vec<i32> = section
        .palette()
        .iter()
        .map(|block| palette::runtime_id(block).unwrap_or(RUNTIME_ID_AIR))
        .collect();

    if section.uniform_block().is_some() || palette.len() <= 1 {
        return Encoded {
            palette,
            indices: None,
        };
    }

    // Storage order and wire order are different by design, so this walks positions and
    // asks for each one rather than copying an array across.
    let mut indices = vec![0u16; BLOCKS_PER_SUBCHUNK];
    for x in 0..SECTION_SIZE {
        for y in 0..SECTION_SIZE {
            for z in 0..SECTION_SIZE {
                if let Some(index) = section.index_at(x, y, z) {
                    indices[block_offset(x, y, z)] = index;
                }
            }
        }
    }

    Encoded {
        palette,
        indices: Some(indices),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_blocks::{AIR, Block, STONE};
    use bedrock_protocol::bytes::Reader;
    use bedrock_protocol::level_chunk::{
        BIOME_PLAINS, OVERWORLD_MIN_SUBCHUNK, OVERWORLD_SUBCHUNKS, RUNTIME_ID_STONE,
    };
    use bedrock_world::World;
    use bedrock_world::chunk::{MAX_SECTION, SECTION_COUNT};

    /// The world's idea of how tall the overworld is and the protocol's have to agree.
    /// They are declared separately — one is storage, one is the wire — so nothing but
    /// a test stops them drifting apart, and the symptom of drift is a column the
    /// client reads past the end of.
    #[test]
    fn the_world_and_the_wire_agree_on_the_shape_of_a_column() {
        assert_eq!(SECTION_COUNT, OVERWORLD_SUBCHUNKS);
        assert_eq!(MIN_SECTION, OVERWORLD_MIN_SUBCHUNK);
        assert_eq!(MAX_SECTION, level_chunk::OVERWORLD_MAX_SUBCHUNK);
    }

    /// Reads the header of a `LevelChunk` back: position, dimension, subchunk count.
    fn header(packet: &Packet) -> (i32, i32, usize) {
        let mut r = Reader::new(&packet.body);
        let x = r.zigzag32().unwrap();
        let z = r.zigzag32().unwrap();
        assert_eq!(r.zigzag32().unwrap(), 0, "overworld");
        let count = r.varint().unwrap() as usize;
        (x, z, count)
    }

    #[test]
    fn a_flat_column_declares_only_the_subchunks_that_hold_ground() {
        let mut world = World::flat(80);
        let packet = column_packet(world.chunk(2, -5), BIOME_PLAINS);

        // Ground stops at y=80, so the topmost used section is the one holding y=79.
        assert_eq!(header(&packet), (2, -5, 9));
    }

    #[test]
    fn an_empty_column_carries_no_subchunks() {
        let mut world = World::flat(bedrock_world::chunk::MIN_Y);
        let packet = column_packet(world.chunk(0, 0), BIOME_PLAINS);
        assert_eq!(header(&packet), (0, 0, 0));
    }

    /// The flat world's sections are uniform, so every one of them must come out as the
    /// six-byte case. Losing that is 4 KiB a section instead of 6 bytes.
    #[test]
    fn uniform_sections_stay_uniform_on_the_wire() {
        let mut world = World::flat(80);
        let packet = column_packet(world.chunk(0, 0), BIOME_PLAINS);
        let (_, _, count) = header(&packet);

        let mut r = Reader::new(&packet.body);
        for _ in 0..3 {
            r.zigzag32().unwrap();
        }
        r.varint().unwrap();
        assert_eq!(r.u8().unwrap(), 0, "no blob hashes");
        let payload = r.prefixed().unwrap();

        let mut r = Reader::new(payload);
        for index in 0..count {
            assert_eq!(r.u8().unwrap(), 8, "version at {index}");
            assert_eq!(r.u8().unwrap(), 1, "layers at {index}");
            assert_eq!(r.u8().unwrap(), 1, "zero bits per block at {index}");
            let id = r.zigzag32().unwrap();
            let expected = if index < 9 {
                RUNTIME_ID_STONE
            } else {
                RUNTIME_ID_AIR
            };
            assert_eq!(id, expected, "block at subchunk {index}");
        }
    }

    /// The exact bytes of a flat column, pinned.
    ///
    /// The version a real client accepted at M0.4 carried **ten** subchunks, the last
    /// of them air. This carries nine and stops at the ground: everything above the
    /// highest used section is sky, and the client draws sky without being told.
    ///
    /// That is the one thing in this change a client can disagree with, so the shape is
    /// written out here rather than described. If a client ever refuses this column,
    /// this test is where the difference is visible.
    #[test]
    fn a_flat_column_is_nine_subchunks_of_stone_then_biomes() {
        let mut expected = Vec::new();
        for _ in 0..9 {
            expected.extend_from_slice(&[8, 1, 1]); // version, one layer, zero bits
            expected.extend_from_slice(&[0xa4, 0x2a]); // zigzag(RUNTIME_ID_STONE)
        }
        for _ in 0..OVERWORLD_SUBCHUNKS {
            expected.extend_from_slice(&[1, 2]); // zero bits, zigzag(BIOME_PLAINS)
        }
        expected.push(0); // border blocks

        let mut world = World::flat(80);
        let packet = column_packet(world.chunk(0, 0), BIOME_PLAINS);

        let mut r = Reader::new(&packet.body);
        for _ in 0..3 {
            r.zigzag32().unwrap();
        }
        assert_eq!(r.varint().unwrap(), 9, "subchunks declared");
        assert_eq!(r.u8().unwrap(), 0, "no blob hashes");
        assert_eq!(r.prefixed().unwrap(), expected.as_slice());
        assert!(r.is_empty(), "and nothing after the payload");
    }

    #[test]
    fn a_section_with_two_blocks_in_it_carries_indices() {
        let section = Section::uniform(STONE).with_block(1, 2, 3, AIR).unwrap();
        let encoded = encode_section(&section);

        assert_eq!(encoded.palette, vec![RUNTIME_ID_STONE, RUNTIME_ID_AIR]);
        let indices = encoded.indices.unwrap();
        assert_eq!(indices.len(), BLOCKS_PER_SUBCHUNK);
        assert_eq!(indices[block_offset(1, 2, 3)], 1, "the air block");
        assert_eq!(
            indices[block_offset(0, 0, 0)],
            0,
            "and stone everywhere else"
        );
    }

    /// A block this version has no id for is written as air rather than as something
    /// else — a hole reads as a missing palette entry, a wrong block reads as a bug.
    #[test]
    fn an_unknown_block_becomes_air() {
        let section = Section::uniform(Block::new("minecraft:sculk_shrieker"));
        assert_eq!(encode_section(&section).palette, vec![RUNTIME_ID_AIR]);
    }
}
