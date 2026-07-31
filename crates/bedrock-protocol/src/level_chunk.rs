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
//! A subchunk names its blocks by runtime id, which is a block state's **index in the
//! version's canonical ordered list**. It is not stable across versions and it cannot
//! be guessed: `minecraft:air` is 13094, not 0. Assuming zero meant air would fill a
//! world with whatever happens to sort first.
//!
//! The ids below were derived with `scripts/block-runtime-id.py`, which reads that list
//! and prints an index. They are constants here rather than an asset because a flat
//! world needs two of sixteen thousand; the script is what keeps them re-derivable
//! instead of magic.
//!
//! # Uniform subchunks cost six bytes
//!
//! Zero bits per block is the single-value case: no word array, no palette count, one
//! entry. A subchunk that is entirely stone or entirely air costs three header bytes
//! and one varint, which is what makes a flat world cheap enough to send eighty-one
//! columns of it without thinking about compression.

use crate::batch::Packet;
use crate::bytes::Writer;

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

/// `minecraft:air`. Not zero — see the module docs.
pub const RUNTIME_ID_AIR: i32 = 13_094;

/// `minecraft:stone`.
pub const RUNTIME_ID_STONE: i32 = 2_706;

/// Blocks tall a subchunk is.
pub const SUBCHUNK_HEIGHT: i32 = 16;

/// Writes one subchunk's worth of biome, all of it the same.
///
/// Zero bits per block is the single-value case: no word array, and no palette count
/// either — the one entry follows the header directly.
fn write_uniform_biome(w: &mut Writer, biome: i32) {
    // (bits_per_block << 1) | runtime, with zero bits per block.
    w.u8(1);
    w.zigzag32(biome);
}

/// Writes a subchunk that is entirely one block.
fn write_uniform_subchunk(w: &mut Writer, runtime_id: i32) {
    w.u8(8) // subchunk version
        .u8(1) // one storage layer
        .u8(1); // (bits_per_block << 1) | runtime, with zero bits per block
    w.zigzag32(runtime_id);
}

/// How many subchunks a column must carry to reach `surface_height`.
///
/// Subchunks are written from the bottom of the dimension upwards with no gaps, so
/// reaching y = 80 means writing every subchunk below it too.
pub fn subchunks_up_to(surface_height: i32) -> usize {
    let top = surface_height.div_euclid(SUBCHUNK_HEIGHT);
    (top - OVERWORLD_MIN_SUBCHUNK + 1).max(0) as usize
}

/// Builds the payload of a flat column: stone up to `surface_height`, air above.
///
/// The surface is the first air block, so a player standing at `surface_height` is
/// standing on stone.
pub fn flat_column(biome: i32, surface_height: i32) -> Vec<u8> {
    let mut w = Writer::new();

    let count = subchunks_up_to(surface_height);
    let surface_subchunk = surface_height.div_euclid(SUBCHUNK_HEIGHT);

    for index in 0..count {
        let y = OVERWORLD_MIN_SUBCHUNK + index as i32;
        let block = if y < surface_subchunk {
            RUNTIME_ID_STONE
        } else {
            RUNTIME_ID_AIR
        };
        write_uniform_subchunk(&mut w, block);
    }

    for _ in 0..OVERWORLD_SUBCHUNKS {
        write_uniform_biome(&mut w, biome);
    }

    w.u8(0); // border blocks: the client crashes on real entries, and we have none

    // Block entities would follow as raw NBT. There are none.

    w.finish()
}

/// Builds the payload of an all-air column.
pub fn void_column(biome: i32) -> Vec<u8> {
    let mut w = Writer::new();

    // No subchunks: nothing to name, nothing to write.

    for _ in 0..OVERWORLD_SUBCHUNKS {
        write_uniform_biome(&mut w, biome);
    }

    w.u8(0);
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
pub fn publisher_update(x: i32, y: i32, z: i32, radius: u32) -> Packet {
    let mut w = Writer::new();
    w.zigzag32(x).zigzag32(y).zigzag32(z).varint(radius).u32(0); // no saved chunks

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

    /// Two bytes per subchunk: the header, then the single palette entry. No word
    /// array and no count, because zero bits per block means one value everywhere.
    #[test]
    fn a_void_column_is_biomes_and_a_border_count() {
        let payload = void_column(BIOME_PLAINS);
        assert_eq!(payload.len(), OVERWORLD_SUBCHUNKS * 2 + 1);
        assert_eq!(*payload.last().unwrap(), 0, "border block count");
    }

    #[test]
    fn every_subchunk_carries_the_biome() {
        let payload = void_column(BIOME_PLAINS);
        let mut r = Reader::new(&payload);
        for index in 0..OVERWORLD_SUBCHUNKS {
            assert_eq!(r.u8().unwrap(), 1, "header at {index}");
            assert_eq!(r.zigzag32().unwrap(), BIOME_PLAINS, "biome at {index}");
        }
        assert_eq!(r.u8().unwrap(), 0);
        assert!(r.is_empty());
    }

    /// The count is zero because naming even one block needs a palette this server does
    /// not have. A non-zero count without the subchunks would truncate the column.
    #[test]
    fn a_void_column_declares_no_subchunks() {
        let packet = level_chunk(0, 0, 0, &void_column(BIOME_PLAINS));
        let mut r = Reader::new(&packet.body);
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        assert_eq!(r.varint().unwrap(), 0);
    }

    /// Subchunks are written from the bottom up with no gaps, so a surface at y=80
    /// means every subchunk from y=-64 to y=80 has to be there.
    #[test]
    fn reaching_a_surface_means_writing_everything_below_it() {
        assert_eq!(subchunks_up_to(-64), 1, "just the bottom one");
        assert_eq!(subchunks_up_to(0), 5, "-64..0 is five subchunks");
        assert_eq!(subchunks_up_to(80), 10);
    }

    /// Three header bytes and one varint each. The varint is not a fixed width — a
    /// runtime id of 13094 costs three bytes where 2706 costs two — so the saving is
    /// real but not a round number: ten subchunks of flat world in fifty-one bytes.
    #[test]
    fn a_uniform_subchunk_costs_a_header_and_one_id() {
        let count = subchunks_up_to(80);
        let payload = flat_column(BIOME_PLAINS, 80);
        let blocks = payload.len() - (OVERWORLD_SUBCHUNKS * 2 + 1);

        assert!(blocks >= count * 4, "at least a header and one byte each");
        assert!(blocks <= count * 8, "and nowhere near a word array");
    }

    /// Stone below the surface, air at and above it, so a player at the surface height
    /// is standing on something.
    #[test]
    fn the_surface_is_where_stone_stops() {
        let payload = flat_column(BIOME_PLAINS, 80);
        let mut r = Reader::new(&payload);

        for index in 0..subchunks_up_to(80) {
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
        let count = subchunks_up_to(80);
        let packet = level_chunk(0, 0, count, &flat_column(BIOME_PLAINS, 80));

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
        let packet = publisher_update(0, 70, 0, 4);
        assert_eq!(packet.id, ID_NETWORK_CHUNK_PUBLISHER_UPDATE);

        let mut r = Reader::new(&packet.body);
        assert_eq!(r.zigzag32().unwrap(), 0);
        assert_eq!(r.zigzag32().unwrap(), 70);
        assert_eq!(r.zigzag32().unwrap(), 0);
        assert_eq!(r.varint().unwrap(), 4);
        assert_eq!(r.u32().unwrap(), 0, "no saved chunks");
    }

    #[test]
    fn a_radius_covers_the_square_around_its_centre() {
        assert_eq!(columns_around(0, 0, 0), vec![(0, 0)]);
        assert_eq!(columns_around(0, 0, 1).len(), 9);
        assert_eq!(columns_around(0, 0, 4).len(), 81);
        assert!(columns_around(5, -3, 2).contains(&(5, -3)));
    }
}
