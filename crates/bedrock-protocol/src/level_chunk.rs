//! `LevelChunk`: a column of the world.
//!
//! ```text
//! zigzag x, zigzag z    which column
//! zigzag dimension
//! varint subchunk count
//! bool   blob hashes    always false here
//! string payload        the column itself, length-prefixed
//! ```
//!
//! # Naming a block
//!
//! A subchunk names its blocks by runtime id — see [`crate::palette`], which owns the
//! mapping. Nothing here decides what a block is; it decides how a cube of them is
//! written.
//!
//! # Uniform subchunks cost six bytes
//!
//! Zero bits per block is the single-value case: no word array, no palette count, one
//! entry. A subchunk that is entirely stone or entirely air costs three header bytes
//! and one varint, which is what makes a flat world cheap enough to send eighty-one
//! columns of it without thinking about compression.

use crate::batch::Packet;
use crate::bytes::Writer;
pub use crate::palette::{RUNTIME_ID_AIR, RUNTIME_ID_STONE};

/// `LevelChunk`, server to client.
pub const ID_LEVEL_CHUNK: u32 = 58;

/// `NetworkChunkPublisherUpdate`, server to client.
pub const ID_NETWORK_CHUNK_PUBLISHER_UPDATE: u32 = 121;

/// The overworld's lowest subchunk index, at y = -64.
pub const OVERWORLD_MIN_SUBCHUNK: i32 = -4;

/// The overworld's highest subchunk index, at y = 320.
pub const OVERWORLD_MAX_SUBCHUNK: i32 = 19;

/// Subchunks a full overworld column covers.
pub const OVERWORLD_SUBCHUNKS: usize =
    (OVERWORLD_MAX_SUBCHUNK - OVERWORLD_MIN_SUBCHUNK + 1) as usize;

/// Plains. Numeric biome ids are stable across versions, unlike block ids.
pub const BIOME_PLAINS: i32 = 1;

/// Blocks tall a subchunk is.
pub const SUBCHUNK_HEIGHT: i32 = 16;

/// Blocks per chunk along X and Z. Same number as [`SUBCHUNK_HEIGHT`], different axis.
pub const CHUNK_WIDTH: i32 = 16;

/// Writes one subchunk's worth of biome, all of it the same.
///
/// Zero bits per block is the single-value case: no word array, and no palette count
/// either — the one entry follows the header directly.
fn write_uniform_biome(w: &mut Writer, biome: i32) {
    // (bits_per_block << 1) | runtime, with zero bits per block.
    w.u8(1);
    w.zigzag32(biome);
}

/// Blocks in one subchunk.
pub const BLOCKS_PER_SUBCHUNK: usize = (SUBCHUNK_HEIGHT * CHUNK_WIDTH * CHUNK_WIDTH) as usize;

/// Where a position sits in a subchunk's index array.
///
/// X first, then Z, then Y — the client reads them in that order, and a different one
/// builds a world that is a transposed copy of the one intended. Storage has its own
/// order and this is deliberately not it: the two meet in `bedrock-server`, which asks
/// for blocks by coordinate.
pub fn block_offset(x: usize, y: usize, z: usize) -> usize {
    (x << 8) | (z << 4) | y
}

/// One subchunk, ready to write.
///
/// `palette` holds runtime ids. `indices` holds one palette index per position, in
/// [`block_offset`] order — or `None` when the whole cube is `palette[0]`, which is the
/// case that costs six bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subchunk<'a> {
    /// The distinct runtime ids in this cube. Empty is treated as all air.
    pub palette: &'a [i32],
    /// One palette index per position, or `None` for a cube of a single block.
    pub indices: Option<&'a [u16]>,
}

impl<'a> Subchunk<'a> {
    /// A cube that is one block everywhere.
    pub fn uniform(palette: &'a [i32]) -> Self {
        Self {
            palette,
            indices: None,
        }
    }
}

/// How many bits an index needs, rounded up to a width the format allows.
///
/// The client only reads these widths; 7 bits is not a thing, and writing it produces
/// a word array it unpacks as garbage.
fn bits_per_block(palette_len: usize) -> u8 {
    match palette_len {
        0..=1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        17..=32 => 5,
        33..=64 => 6,
        65..=256 => 8,
        _ => 16,
    }
}

/// Writes a subchunk: header, block data, palette.
fn write_subchunk(w: &mut Writer, subchunk: &Subchunk<'_>) {
    w.u8(8) // subchunk version
        .u8(1); // one storage layer

    let uniform = subchunk.indices.is_none() || subchunk.palette.len() <= 1;
    if uniform {
        write_uniform_storage(w, subchunk.palette.first().copied());
        return;
    }

    let bits = bits_per_block(subchunk.palette.len());
    let indices = subchunk.indices.unwrap_or(&[]);

    // (bits_per_block << 1) | runtime
    w.u8((bits << 1) | 1);

    // At least one bit per block here: the zero-bit case took the branch above.
    let per_word = 32 / bits.max(1) as usize;
    let words = BLOCKS_PER_SUBCHUNK.div_ceil(per_word);
    for word in 0..words {
        let mut packed = 0u32;
        for slot in 0..per_word {
            let position = word * per_word + slot;
            if position >= BLOCKS_PER_SUBCHUNK {
                break;
            }
            let index = u32::from(indices.get(position).copied().unwrap_or(0));
            packed |= index << (slot * bits as usize);
        }
        w.u32(packed);
    }

    w.zigzag32(i32::try_from(subchunk.palette.len()).unwrap_or(1));
    for &id in subchunk.palette {
        w.zigzag32(id);
    }
}

/// Writes the single-value storage: no word array, no count, one entry.
fn write_uniform_storage(w: &mut Writer, runtime_id: Option<i32>) {
    w.u8(1); // (bits_per_block << 1) | runtime, with zero bits per block
    w.zigzag32(runtime_id.unwrap_or(RUNTIME_ID_AIR));
}

/// Builds the payload of a column: its subchunks bottom-first, then biomes, then the
/// border-block count.
///
/// Subchunks are written from the bottom of the dimension upwards with no gaps, so a
/// caller that wants a surface at y = 80 passes every subchunk below it too. The count
/// the `LevelChunk` header declares must be `subchunks.len()`.
pub fn column_payload(subchunks: &[Subchunk<'_>], biome: i32) -> Vec<u8> {
    let mut w = Writer::new();

    for subchunk in subchunks {
        write_subchunk(&mut w, subchunk);
    }

    // Biomes cover the whole dimension regardless of how many subchunks carry blocks.
    for _ in 0..OVERWORLD_SUBCHUNKS {
        write_uniform_biome(&mut w, biome);
    }

    w.u8(0); // border blocks: the client crashes on real entries, and we have none

    // Block entities would follow as raw NBT. There are none.

    w.finish()
}

/// Builds a `LevelChunk` for one column.
///
/// `subchunks` must match how many the payload actually carries: the count tells the
/// client how much of the payload is blocks and where the biomes begin, so a wrong
/// number reads biomes as block data.
pub fn level_chunk(chunk_x: i32, chunk_z: i32, subchunks: usize, payload: &[u8]) -> Packet {
    let mut w = Writer::new();
    w.zigzag32(chunk_x)
        .zigzag32(chunk_z)
        .zigzag32(0) // dimension: overworld
        .varint(u32::try_from(subchunks).unwrap_or(0))
        .u8(0) // no blob hashes
        .prefixed(payload);

    Packet::new(ID_LEVEL_CHUNK, w.finish())
}

/// Builds `NetworkChunkPublisherUpdate`.
///
/// Tells the client which point to accept chunks around. Without it a client discards
/// columns it did not expect, and the world stays empty however many are sent.
///
/// The radius is in **blocks**, not chunks. Passing a chunk count shrinks the accept
/// area by 16x, which discards every column but the one under the player — silently,
/// with no disconnect and no complaint.
pub fn publisher_update(x: i32, y: i32, z: i32, radius_blocks: u32) -> Packet {
    let mut w = Writer::new();
    w.zigzag32(x)
        .zigzag32(y)
        .zigzag32(z)
        .varint(radius_blocks)
        .u32(0); // no saved chunks

    Packet::new(ID_NETWORK_CHUNK_PUBLISHER_UPDATE, w.finish())
}

/// Every column within `radius` of a centre, in chunk coordinates.
pub fn columns_around(centre_x: i32, centre_z: i32, radius: i32) -> Vec<(i32, i32)> {
    let mut out = Vec::new();
    for x in (centre_x - radius)..=(centre_x + radius) {
        for z in (centre_z - radius)..=(centre_z + radius) {
            out.push((x, z));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Reader;

    #[test]
    fn the_overworld_spans_twenty_four_subchunks() {
        assert_eq!(OVERWORLD_SUBCHUNKS, 24);
    }

    /// A column of nothing: no subchunks, biomes, and the border count.
    #[test]
    fn an_empty_column_is_biomes_and_a_border_count() {
        let payload = column_payload(&[], BIOME_PLAINS);
        assert_eq!(payload.len(), OVERWORLD_SUBCHUNKS * 2 + 1);
        assert_eq!(*payload.last().unwrap(), 0, "border block count");
    }

    #[test]
    fn every_subchunk_carries_the_biome() {
        let payload = column_payload(&[], BIOME_PLAINS);
        let mut r = Reader::new(&payload);
        for index in 0..OVERWORLD_SUBCHUNKS {
            assert_eq!(r.u8().unwrap(), 1, "header at {index}");
            assert_eq!(r.zigzag32().unwrap(), BIOME_PLAINS, "biome at {index}");
        }
        assert_eq!(r.u8().unwrap(), 0);
        assert!(r.is_empty());
    }

    /// The count is what tells the client where blocks end and biomes begin, so a
    /// column with nothing in it has to declare zero rather than be truncated.
    #[test]
    fn an_empty_column_declares_no_subchunks() {
        let packet = level_chunk(0, 0, 0, &column_payload(&[], BIOME_PLAINS));
        let mut r = Reader::new(&packet.body);
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        assert_eq!(r.varint().unwrap(), 0);
    }

    /// Three header bytes and one varint each. The varint is not a fixed width — a
    /// runtime id of 13094 costs three bytes where 2706 costs two — so the saving is
    /// real but not a round number: ten subchunks of flat world in fifty-one bytes.
    #[test]
    fn a_uniform_subchunk_costs_a_header_and_one_id() {
        let stone = [RUNTIME_ID_STONE];
        let subchunks = vec![Subchunk::uniform(&stone); 10];
        let payload = column_payload(&subchunks, BIOME_PLAINS);
        let blocks = payload.len() - (OVERWORLD_SUBCHUNKS * 2 + 1);

        assert!(blocks >= subchunks.len() * 4, "a header and one id each");
        assert!(blocks <= subchunks.len() * 8, "nowhere near a word array");
    }

    /// Stone below the surface, air at and above it, so a player at the surface height
    /// is standing on something.
    #[test]
    fn a_column_writes_its_subchunks_bottom_first() {
        let stone = [RUNTIME_ID_STONE];
        let air = [RUNTIME_ID_AIR];
        let subchunks: Vec<Subchunk<'_>> = (OVERWORLD_MIN_SUBCHUNK..5)
            .map(|_| Subchunk::uniform(&stone))
            .chain(std::iter::once(Subchunk::uniform(&air)))
            .collect();

        let payload = column_payload(&subchunks, BIOME_PLAINS);
        let mut r = Reader::new(&payload);

        for (index, _) in subchunks.iter().enumerate() {
            assert_eq!(r.u8().unwrap(), 8, "version at {index}");
            assert_eq!(r.u8().unwrap(), 1, "layers at {index}");
            assert_eq!(r.u8().unwrap(), 1, "zero bits per block at {index}");

            let y = OVERWORLD_MIN_SUBCHUNK + index as i32;
            let expected = if y < 5 {
                RUNTIME_ID_STONE
            } else {
                RUNTIME_ID_AIR
            };
            assert_eq!(r.zigzag32().unwrap(), expected, "block at subchunk {y}");
        }
    }

    /// The mixed case: a word array, then the palette. Reading it back is the only way
    /// to know the packing is right, because a client that disagrees says nothing —
    /// it draws the wrong world.
    #[test]
    fn a_mixed_subchunk_packs_indices_and_then_the_palette() {
        let palette = [RUNTIME_ID_AIR, RUNTIME_ID_STONE];
        let mut indices = vec![0u16; BLOCKS_PER_SUBCHUNK];
        indices[block_offset(3, 4, 5)] = 1;
        indices[block_offset(15, 15, 15)] = 1;

        let payload = column_payload(
            &[Subchunk {
                palette: &palette,
                indices: Some(&indices),
            }],
            BIOME_PLAINS,
        );

        let mut r = Reader::new(&payload);
        assert_eq!(r.u8().unwrap(), 8, "version");
        assert_eq!(r.u8().unwrap(), 1, "layers");
        assert_eq!(r.u8().unwrap(), (1 << 1) | 1, "one bit per block, runtime");

        // 32 blocks a word with one bit each: 128 words cover the cube.
        let mut read_back = vec![0u16; BLOCKS_PER_SUBCHUNK];
        for word in 0..BLOCKS_PER_SUBCHUNK / 32 {
            let packed = r.u32().unwrap();
            for slot in 0..32 {
                read_back[word * 32 + slot] = ((packed >> slot) & 1) as u16;
            }
        }
        assert_eq!(read_back, indices, "every block came back where it was put");

        assert_eq!(r.zigzag32().unwrap(), 2, "palette size");
        assert_eq!(r.zigzag32().unwrap(), RUNTIME_ID_AIR);
        assert_eq!(r.zigzag32().unwrap(), RUNTIME_ID_STONE);
    }

    /// Widths the format does not have produce a word array the client unpacks as
    /// garbage, so a palette of 5 rounds up to 3 bits rather than using 3 exactly.
    #[test]
    fn index_widths_round_up_to_ones_the_client_reads() {
        assert_eq!(bits_per_block(1), 0, "single value: no array at all");
        assert_eq!(bits_per_block(2), 1);
        assert_eq!(bits_per_block(3), 2);
        assert_eq!(bits_per_block(5), 3);
        assert_eq!(bits_per_block(17), 5);
        assert_eq!(bits_per_block(65), 8);
        assert_eq!(bits_per_block(257), 16);
        for len in 1..=4096 {
            let bits = bits_per_block(len);
            assert!(
                matches!(bits, 0 | 1 | 2 | 3 | 4 | 5 | 6 | 8 | 16),
                "{len} blocks asked for {bits} bits"
            );
        }
    }

    /// Every position must land in its own slot, or two blocks share one and the world
    /// comes out with holes in it.
    #[test]
    fn every_position_has_its_own_slot_on_the_wire() {
        let mut seen = std::collections::HashSet::new();
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    let offset = block_offset(x, y, z);
                    assert!(offset < BLOCKS_PER_SUBCHUNK);
                    assert!(seen.insert(offset), "{x},{y},{z} repeats");
                }
            }
        }
        assert_eq!(seen.len(), BLOCKS_PER_SUBCHUNK);
    }

    /// A subchunk that declares a palette but hands over no indices is the uniform
    /// case, and writing a word array for it would desynchronise the whole column.
    #[test]
    fn a_palette_without_indices_is_written_as_one_block() {
        let palette = [RUNTIME_ID_STONE, RUNTIME_ID_AIR];
        let payload = column_payload(
            &[Subchunk {
                palette: &palette,
                indices: None,
            }],
            BIOME_PLAINS,
        );

        let mut r = Reader::new(&payload);
        assert_eq!(r.u8().unwrap(), 8);
        assert_eq!(r.u8().unwrap(), 1);
        assert_eq!(r.u8().unwrap(), 1, "zero bits per block");
        assert_eq!(r.zigzag32().unwrap(), RUNTIME_ID_STONE);
    }

    /// Air is not zero. Guessing that it was would fill a world with whatever sorts
    /// first in the version's list.
    #[test]
    fn air_is_not_block_zero() {
        assert_ne!(RUNTIME_ID_AIR, 0);
        assert_ne!(RUNTIME_ID_AIR, RUNTIME_ID_STONE);
    }

    /// The count tells the client where blocks end and biomes begin, so it has to match
    /// what the payload carries.
    #[test]
    fn the_declared_count_matches_the_payload() {
        let stone = [RUNTIME_ID_STONE];
        let subchunks = vec![Subchunk::uniform(&stone); 10];
        let count = subchunks.len();
        let packet = level_chunk(0, 0, count, &column_payload(&subchunks, BIOME_PLAINS));

        let mut r = Reader::new(&packet.body);
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        assert_eq!(r.varint().unwrap() as usize, count);
    }

    #[test]
    fn a_column_round_trips_its_position() {
        for (x, z) in [(0, 0), (-1, 5), (100, -100)] {
            let packet = level_chunk(x, z, 0, b"payload");
            assert_eq!(packet.id, ID_LEVEL_CHUNK);

            let mut r = Reader::new(&packet.body);
            assert_eq!(r.zigzag32().unwrap(), x);
            assert_eq!(r.zigzag32().unwrap(), z);
            assert_eq!(r.zigzag32().unwrap(), 0, "overworld");
            r.varint().unwrap();
            assert_eq!(r.u8().unwrap(), 0, "no blob hashes");
            assert_eq!(r.prefixed().unwrap(), b"payload");
        }
    }

    #[test]
    fn the_publisher_update_carries_its_centre_and_radius() {
        let packet = publisher_update(0, 80, 0, 64);
        assert_eq!(packet.id, ID_NETWORK_CHUNK_PUBLISHER_UPDATE);

        let mut r = Reader::new(&packet.body);
        assert_eq!(r.zigzag32().unwrap(), 0);
        assert_eq!(r.zigzag32().unwrap(), 80);
        assert_eq!(r.zigzag32().unwrap(), 0);
        assert_eq!(r.varint().unwrap(), 64);
        assert_eq!(r.u32().unwrap(), 0, "no saved chunks");
    }

    #[test]
    fn the_publisher_radius_reaches_the_columns_we_send() {
        let chunks = 4;
        let packet = publisher_update(0, 80, 0, (chunks * CHUNK_WIDTH) as u32);

        let mut r = Reader::new(&packet.body);
        for _ in 0..3 {
            r.zigzag32().unwrap();
        }
        let radius = r.varint().unwrap() as i32;

        let furthest = columns_around(0, 0, chunks)
            .iter()
            .map(|&(x, z)| x.abs().max(z.abs()) * CHUNK_WIDTH)
            .max()
            .unwrap();

        // A radius in chunk units falls 16x short. The client then drops every column
        // but the one under the player, without disconnecting or complaining.
        assert!(radius >= furthest, "radius {radius} must reach {furthest}");
    }

    #[test]
    fn a_radius_covers_the_square_around_its_centre() {
        assert_eq!(columns_around(0, 0, 0), vec![(0, 0)]);
        assert_eq!(columns_around(0, 0, 1).len(), 9);
        assert_eq!(columns_around(0, 0, 4).len(), 81);
        assert!(columns_around(5, -3, 2).contains(&(5, -3)));
    }
}
