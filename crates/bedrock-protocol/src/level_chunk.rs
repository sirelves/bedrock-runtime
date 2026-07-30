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
//! # Why these columns are empty
//!
//! A subchunk names its blocks by runtime id — an index into the version's block
//! palette, which this server does not have and deliberately does not dump. Writing
//! `stone` would mean guessing a number.
//!
//! A column with **zero** subchunks needs no ids at all: it is air from top to bottom.
//! Biomes are still written, because they are numeric ids that do not depend on a
//! palette, and the client wants one for every subchunk in the dimension whether or not
//! blocks were sent for it.
//!
//! That gives a client a real world to stand in, made of nothing. Solid ground waits
//! for block ids the server can name.

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

/// Writes one subchunk's worth of biome, all of it the same.
///
/// Zero bits per block is the single-value case: no word array, and no palette count
/// either — the one entry follows the header directly.
fn write_uniform_biome(w: &mut Writer, biome: i32) {
    // (bits_per_block << 1) | runtime, with zero bits per block.
    w.u8(1);
    w.zigzag32(biome);
}

/// Builds the payload of an all-air column.
pub fn void_column(biome: i32) -> Vec<u8> {
    let mut w = Writer::new();

    // No subchunks: nothing to name, nothing to write.

    for _ in 0..OVERWORLD_SUBCHUNKS {
        write_uniform_biome(&mut w, biome);
    }

    w.u8(0); // border blocks: the client crashes on real entries, and we have none

    // Block entities would follow as raw NBT. There are none.

    w.finish()
}

/// Builds a `LevelChunk` for one column.
pub fn level_chunk(chunk_x: i32, chunk_z: i32, payload: &[u8]) -> Packet {
    let mut w = Writer::new();
    w.zigzag32(chunk_x)
        .zigzag32(chunk_z)
        .zigzag32(0) // dimension: overworld
        .varint(0) // subchunk count
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
    fn the_subchunk_count_is_zero() {
        let packet = level_chunk(0, 0, &void_column(BIOME_PLAINS));
        let mut r = Reader::new(&packet.body);
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        r.zigzag32().unwrap();
        assert_eq!(r.varint().unwrap(), 0);
    }

    #[test]
    fn a_column_round_trips_its_position() {
        for (x, z) in [(0, 0), (-1, 5), (100, -100)] {
            let packet = level_chunk(x, z, b"payload");
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
