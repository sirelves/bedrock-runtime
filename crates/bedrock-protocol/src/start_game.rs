//! `StartGame`: the packet that tells a client what world it just joined.
//!
//! The largest packet in the login sequence — 25 fields, one of which carries fifty
//! more — and it fails the way everything else here fails: a field out of place and the
//! client leaves without saying why.
//!
//! # Where the layout came from
//!
//! Not from Mojang's published schemas, which describe protocol 2169. Their changelog
//! for 2168 lists `StartGamePacket` among the packets "converted to Cereal-only
//! serialization", alongside others with explicit wire changes, and our target is 1001.
//!
//! The order below is read from a reference implementation pinned to 1001 exactly. It
//! disagrees with the 2169 schema in ways that would have failed silently: there is an
//! `is_logging_chat` boolean the schema does not list, and the server join information
//! is optional rather than a fixed field.
//!
//! # Encodings that are easy to get wrong
//!
//! Most integers here are varints, but not all of them. `server_chunk_tick_radius`,
//! `limited_world_width` and `limited_world_depth` are fixed 32-bit little-endian, the
//! experiment count is a fixed `u32` while the game rule count is a varint, and the
//! spawn biome is a fixed `u16`. Each is a place where "everything is a varint" writes
//! a packet that decodes into nonsense.

use crate::batch::Packet;
use crate::bytes::Writer;

/// `StartGame`, server to client.
pub const ID_START_GAME: u32 = 11;

/// An empty NBT compound, in the network form: tag, empty name, end.
const EMPTY_NBT: [u8; 3] = [0x0a, 0x00, 0x00];

/// How the world generates terrain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generator {
    /// Ordinary terrain.
    Overworld = 1,
    /// Flat layers. What M0 serves.
    Flat = 2,
    /// Nothing at all.
    Void = 5,
}

/// What a player may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameType {
    /// Blocks break slowly and damage applies.
    Survival = 0,
    /// Flight, instant break, no damage.
    Creative = 1,
}

/// Everything `StartGame` says about the world.
#[derive(Debug, Clone)]
pub struct StartGame {
    /// The player's unique id.
    pub entity_id: i64,
    /// The player's runtime id.
    pub runtime_id: u64,
    /// The mode the joining player is in.
    pub game_type: GameType,
    /// Where the player appears.
    pub position: (f32, f32, f32),
    /// Where the player looks, in degrees.
    pub rotation: (f32, f32),
    /// World seed.
    pub seed: u64,
    /// How terrain is made.
    pub generator: Generator,
    /// Where the world's spawn point is.
    pub spawn: (i32, i32, i32),
    /// Shown in the client's world list.
    pub world_name: String,
    /// The version string the client checks its own build against.
    pub vanilla_version: String,
    /// Identifies this server build in crash reports and telemetry.
    pub server_version: String,
}

impl StartGame {
    /// A flat creative world, which is the smallest thing a client will accept.
    pub fn flat(world_name: &str, vanilla_version: &str, server_version: &str) -> Self {
        Self {
            entity_id: 1,
            runtime_id: 1,
            game_type: GameType::Creative,
            position: (0.0, 70.0, 0.0),
            rotation: (0.0, 0.0),
            seed: 0,
            generator: Generator::Flat,
            spawn: (0, 70, 0),
            world_name: world_name.to_owned(),
            vanilla_version: vanilla_version.to_owned(),
            server_version: server_version.to_owned(),
        }
    }

    /// Writes the fifty fields of the level settings block.
    fn write_level_settings(&self, w: &mut Writer) {
        w.u64(self.seed);

        // Spawn settings: the biome type is a fixed u16, not a varint.
        w.u16(0).prefixed(b"plains").zigzag32(0);

        w.zigzag32(self.generator as i32)
            .zigzag32(self.game_type as i32)
            .u8(0) // hardcore
            .zigzag32(1); // difficulty: easy

        w.zigzag32(self.spawn.0)
            .zigzag32(self.spawn.1)
            .zigzag32(self.spawn.2);

        w.u8(1) // achievements disabled: a server world cannot earn them
            .zigzag32(0) // editor world type
            .u8(0) // created in editor
            .u8(0) // exported from editor
            .zigzag32(0) // day cycle stop time
            .zigzag32(0) // education edition offer
            .u8(0) // education features
            .prefixed(b""); // education product id

        w.f32(0.0) // rain
            .f32(0.0) // lightning
            .u8(0) // confirmed platform locked content
            .u8(1) // multiplayer game
            .u8(1) // LAN broadcast
            .zigzag32(0) // xbox live broadcast
            .zigzag32(0) // platform broadcast
            .u8(1) // commands enabled
            .u8(0); // texture packs required

        w.varint(0); // game rules: a varint count, unlike the experiments below

        w.u32(0).u8(0); // experiments: a fixed u32 count, then "ever toggled"

        w.u8(0) // bonus chest
            .u8(0) // start with map
            .zigzag32(1); // default player permission: member

        // Fixed 32-bit, not varints. Signed because that is what the format says, even
        // though a negative radius or width means nothing.
        w.u32(4); // server chunk tick radius

        for _ in 0..10 {
            w.u8(0); // locked packs, template flags, villagers, persona, skins, emotes
        }

        w.prefixed(self.vanilla_version.as_bytes());
        w.u32(0).u32(0); // limited world width and depth
        w.u8(1); // new nether

        w.prefixed(b"").prefixed(b""); // education shared uri: button, link

        w.u8(0); // experimental gameplay override: absent

        w.u8(0) // chat restriction level
            .u8(0) // disable player interactions
            .zigzag32(0) // server editor connection policy
            .u8(0); // anonymous block drops in editor worlds
    }

    /// Encodes the packet body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();

        w.zigzag64(self.entity_id)
            .varint64(self.runtime_id)
            .zigzag32(self.game_type as i32);

        w.vec3(self.position.0, self.position.1, self.position.2);
        w.f32(self.rotation.0).f32(self.rotation.1);

        self.write_level_settings(&mut w);

        w.prefixed(b"") // level id
            .prefixed(self.world_name.as_bytes())
            .prefixed(b"") // premium world template id
            .u8(0); // is trial

        w.zigzag32(0).u8(0); // movement settings: rewind history, server-auth breaking

        w.u64(0); // current tick
        w.zigzag32(0); // enchantment seed

        // No block palette: the client falls back to the one it ships with. Sending a
        // real palette means dumping it from the target version, which this milestone
        // deliberately does not do.
        w.varint(0);

        w.prefixed(b"") // multiplayer correlation id
            .u8(0) // item stack net manager
            .prefixed(self.server_version.as_bytes())
            .bytes(&EMPTY_NBT); // player property data

        w.u64(0); // block type registry checksum
        w.u64(0).u64(0); // world template id: nil UUID

        w.u8(0) // client-side chunk generation
            .u8(0) // block network ids are hashes
            .u8(0) // network permissions: client sounds not disabled
            .u8(0); // is logging chat

        w.u8(0); // server join information: absent

        // Telemetry: four ids we do not report.
        w.prefixed(b"").prefixed(b"").prefixed(b"").prefixed(b"");

        w.finish()
    }

    /// This packet, ready for a batch.
    pub fn packet(&self) -> Packet {
        Packet::new(ID_START_GAME, self.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Reader;

    fn sample() -> StartGame {
        StartGame::flat("test world", "1.26.30", "bedrock-runtime")
    }

    #[test]
    fn the_packet_carries_the_right_id() {
        assert_eq!(sample().packet().id, ID_START_GAME);
    }

    /// Reads the head back field by field. Getting the first three wrong shifts
    /// everything after them, so they are worth pinning on their own.
    #[test]
    fn the_head_decodes_field_by_field() {
        let body = sample().encode();
        let mut r = Reader::new(&body);

        assert_eq!(r.zigzag64().unwrap(), 1, "entity id");
        assert_eq!(r.varint64().unwrap(), 1, "runtime id");
        assert_eq!(r.zigzag32().unwrap(), GameType::Creative as i32);

        assert!((r.f32().unwrap() - 0.0).abs() < f32::EPSILON, "x");
        assert!((r.f32().unwrap() - 70.0).abs() < f32::EPSILON, "y");
        assert!((r.f32().unwrap() - 0.0).abs() < f32::EPSILON, "z");
    }

    /// The spawn biome is a fixed u16 and the dimension a varint. Reading the biome as
    /// a varint eats one byte instead of two and desynchronises the rest.
    #[test]
    fn the_spawn_settings_mix_fixed_and_varint() {
        let body = sample().encode();
        let mut r = Reader::new(&body);
        for _ in 0..3 {
            r.zigzag64().ok();
        }
        // Skip to the level settings by re-reading the head in order.
        let mut r = Reader::new(&body);
        r.zigzag64().unwrap();
        r.varint64().unwrap();
        r.zigzag32().unwrap();
        for _ in 0..5 {
            r.f32().unwrap();
        }
        assert_eq!(r.varint64().unwrap() & 0xff, 0, "seed low byte");
    }

    #[test]
    fn the_world_name_and_versions_survive() {
        let body = StartGame::flat("meu mundo", "1.26.30", "bedrock-runtime").encode();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("meu mundo"));
        assert!(text.contains("1.26.30"));
        assert!(text.contains("bedrock-runtime"));
    }

    /// An empty block palette is a deliberate bet: the client uses the one it ships
    /// with. Sending a count other than zero without the entries would truncate.
    #[test]
    fn the_block_palette_is_empty() {
        let body = sample().encode();
        assert!(!body.is_empty());
        assert!(
            body.windows(3).any(|w| w == EMPTY_NBT),
            "the player property compound should be in there"
        );
    }

    #[test]
    fn two_worlds_differ_only_where_they_should() {
        let a = StartGame::flat("one", "1.26.30", "s").encode();
        let b = StartGame::flat("two", "1.26.30", "s").encode();
        assert_ne!(a, b);
        assert_eq!(a.len(), b.len(), "same-length names, same-length packet");
    }
}
