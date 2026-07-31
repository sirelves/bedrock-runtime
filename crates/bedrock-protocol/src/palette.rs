//! Block identity to runtime id.
//!
//! A runtime id is a block state's **index in the version's canonical ordered list**.
//! It is a network identifier, valid only for the protocol version this crate pins
//! (ADR-004), which is why the mapping lives here and not next to the block vocabulary
//! or the world — see ADR-008.
//!
//! The ids were derived with `scripts/block-runtime-id.py`, which reads that list and
//! prints an index. They are constants rather than an asset because a flat world needs
//! two of some sixteen thousand; the script is what keeps them re-derivable instead of
//! magic.
//!
//! There is no table of the other sixteen thousand yet. A block this does not know is
//! not guessed at — [`runtime_id`] answers `None` and the caller decides, which for a
//! chunk serializer means writing air and leaving a hole a player can see, rather than
//! writing some other block and leaving one they cannot explain.

use bedrock_blocks::Block;

/// `minecraft:air`. Not zero — the list is alphabetical-ish, not air-first, and
/// assuming zero meant air would fill a world with whatever happens to sort first.
pub const RUNTIME_ID_AIR: i32 = 13_094;

/// `minecraft:stone`.
pub const RUNTIME_ID_STONE: i32 = 2_706;

/// The runtime id for a block state, if this version's palette is known to carry it.
///
/// Only blocks without state properties can be answered today: a block with properties
/// occupies a different index per combination, and none of those indices have been
/// derived.
pub fn runtime_id(block: &Block) -> Option<i32> {
    if !block.properties().is_empty() {
        return None;
    }
    match block.name() {
        "minecraft:air" => Some(RUNTIME_ID_AIR),
        "minecraft:stone" => Some(RUNTIME_ID_STONE),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_blocks::{AIR, STONE, Value};

    #[test]
    fn the_two_blocks_a_flat_world_needs_have_ids() {
        assert_eq!(runtime_id(&AIR), Some(RUNTIME_ID_AIR));
        assert_eq!(runtime_id(&STONE), Some(RUNTIME_ID_STONE));
    }

    /// Air is not zero. Guessing that it was would fill a world with whatever sorts
    /// first in the version's list.
    #[test]
    fn air_is_not_block_zero() {
        assert_ne!(RUNTIME_ID_AIR, 0);
        assert_ne!(RUNTIME_ID_AIR, RUNTIME_ID_STONE);
    }

    #[test]
    fn a_block_this_version_does_not_know_is_not_guessed_at() {
        assert_eq!(runtime_id(&Block::new("minecraft:diamond_block")), None);
    }

    /// A stair with its properties is a different index from the same stair with
    /// others, and none of those indices are known. Answering the bare name would put
    /// a block down facing the wrong way, which reads as a bug in the world rather
    /// than a gap in this table.
    #[test]
    fn a_state_with_properties_is_refused_rather_than_approximated() {
        let stair = Block::with_properties(
            "minecraft:stone",
            [("upside_down_bit".into(), Value::Bool(true))],
        );
        assert_eq!(runtime_id(&stair), None);
    }
}
