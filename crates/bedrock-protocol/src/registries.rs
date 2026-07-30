//! The registries a client expects between `StartGame` and its first chunk.
//!
//! Items, entity identifiers and biome definitions. A real server fills all three from
//! data dumped out of the game; this one sends them empty, on the same bet that worked
//! for the block palette — the client already ships with definitions and only needs to
//! be told the server is not overriding them.
//!
//! Sending nothing at all is different from sending an empty list. The chunks reference
//! biome ids, and a client that was never told the biome registry exists has to decide
//! what to do with a reference into a table it does not know it has.

use crate::batch::Packet;
use crate::bytes::Writer;

/// `AvailableActorIdentifiers`, server to client.
pub const ID_AVAILABLE_ACTOR_IDENTIFIERS: u32 = 119;

/// `BiomeDefinitionList`, server to client.
pub const ID_BIOME_DEFINITION_LIST: u32 = 122;

/// `ItemRegistry`, server to client.
pub const ID_ITEM_REGISTRY: u32 = 162;

/// An empty NBT compound in network form: tag, empty name, end.
const EMPTY_NBT: [u8; 3] = [0x0a, 0x00, 0x00];

/// An item registry that overrides nothing.
pub fn empty_item_registry() -> Packet {
    let mut w = Writer::new();
    w.varint(0);
    Packet::new(ID_ITEM_REGISTRY, w.finish())
}

/// Entity identifiers, as a bare NBT compound rather than a counted list.
pub fn empty_actor_identifiers() -> Packet {
    Packet::new(ID_AVAILABLE_ACTOR_IDENTIFIERS, EMPTY_NBT.to_vec())
}

/// Biome definitions: a list of definitions and a list of the strings they index into.
///
/// Both empty. The client keeps its own definitions, which is what makes the biome ids
/// in a chunk mean something without this server dumping a table.
pub fn empty_biome_definitions() -> Packet {
    let mut w = Writer::new();
    w.varint(0).varint(0);
    Packet::new(ID_BIOME_DEFINITION_LIST, w.finish())
}

/// The three, in the order a real server sends them.
pub fn all_empty() -> [Packet; 3] {
    [
        empty_item_registry(),
        empty_actor_identifiers(),
        empty_biome_definitions(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Reader;

    #[test]
    fn an_empty_item_registry_is_one_byte() {
        let packet = empty_item_registry();
        assert_eq!(packet.id, ID_ITEM_REGISTRY);
        assert_eq!(packet.body, vec![0]);
    }

    /// Not a counted list like the others: this one is a bare NBT compound.
    #[test]
    fn actor_identifiers_are_a_bare_compound() {
        let packet = empty_actor_identifiers();
        assert_eq!(packet.id, ID_AVAILABLE_ACTOR_IDENTIFIERS);
        assert_eq!(packet.body, EMPTY_NBT);
    }

    #[test]
    fn biome_definitions_carry_two_empty_lists() {
        let packet = empty_biome_definitions();
        assert_eq!(packet.id, ID_BIOME_DEFINITION_LIST);

        let mut r = Reader::new(&packet.body);
        assert_eq!(r.varint().unwrap(), 0, "definitions");
        assert_eq!(r.varint().unwrap(), 0, "strings");
        assert!(r.is_empty());
    }

    /// Order matters to a client walking its own state machine.
    #[test]
    fn they_go_out_in_the_order_a_real_server_uses() {
        let ids: Vec<u32> = all_empty().iter().map(|p| p.id).collect();
        assert_eq!(
            ids,
            vec![
                ID_ITEM_REGISTRY,
                ID_AVAILABLE_ACTOR_IDENTIFIERS,
                ID_BIOME_DEFINITION_LIST
            ]
        );
    }
}
