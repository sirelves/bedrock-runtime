//! Resource pack negotiation: the step between a login being accepted and the world
//! being described.
//!
//! The server offers packs, the client answers, the server sends the final stack, the
//! client answers again. A server with no packs still has to say so — the client waits
//! for both messages before it will accept a `StartGame`.
//!
//! Layouts follow Mojang's published schemas. Ordinal order is theirs.

use crate::batch::Packet;
use crate::bytes::{DecodeError, Reader, Writer};

/// `ResourcePacksInfo`, server to client.
pub const ID_RESOURCE_PACKS_INFO: u32 = 6;

/// `ResourcePackStack`, server to client.
pub const ID_RESOURCE_PACK_STACK: u32 = 7;

/// `ResourcePackClientResponse`, client to server.
pub const ID_RESOURCE_PACK_CLIENT_RESPONSE: u32 = 8;

/// What the client says about the packs it was offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Response {
    /// The client is leaving.
    Cancel = 0,
    /// The client is fetching packs.
    Downloading = 1,
    /// The client has everything from [`ID_RESOURCE_PACKS_INFO`].
    DownloadingFinished = 2,
    /// The client has applied the stack and is ready for the world.
    StackFinished = 3,
}

impl Response {
    /// Reads the wire value.
    pub fn from_value(value: u8) -> Option<Self> {
        Some(match value {
            0 => Self::Cancel,
            1 => Self::Downloading,
            2 => Self::DownloadingFinished,
            3 => Self::StackFinished,
            _ => return None,
        })
    }
}

/// Decodes a `ResourcePackClientResponse` body.
pub fn decode_response(body: &[u8]) -> Result<Option<Response>, DecodeError> {
    Ok(Response::from_value(Reader::new(body).u8()?))
}

/// A `ResourcePacksInfo` offering nothing.
///
/// The world-template slot still has to carry a UUID and a version string; a nil UUID
/// and an empty version are how "there is no template" is spelled.
pub fn packs_info_empty() -> Packet {
    let mut w = Writer::new();
    w.u8(0) // resource packs required
        .u8(0) // has addon packs
        .u8(0) // has scripts
        .u8(0); // force disable vibrant visuals

    // World template: nil UUID, empty version.
    w.u64(0).u64(0).prefixed(b"");

    w.u16(0); // no resource packs

    Packet::new(ID_RESOURCE_PACKS_INFO, w.finish())
}

/// A `ResourcePackStack` applying nothing.
///
/// `base_game_version` is what the client compares its own build against when deciding
/// whether the stack makes sense; `*` means "whatever this client is".
pub fn pack_stack_empty(base_game_version: &str) -> Packet {
    let mut w = Writer::new();
    w.u8(0); // texture pack required
    w.varint(0); // no texture packs
    w.prefixed(base_game_version.as_bytes());
    // The experiment count is a fixed u32, unlike the pack count above it. Writing a
    // varint here is three bytes short and everything after it reads off the end.
    w.u32(0).u8(0); // no experiment toggles, never toggled
    w.u8(0); // no editor packs

    Packet::new(ID_RESOURCE_PACK_STACK, w.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_response_round_trips() {
        for (value, expected) in [
            (0, Response::Cancel),
            (1, Response::Downloading),
            (2, Response::DownloadingFinished),
            (3, Response::StackFinished),
        ] {
            assert_eq!(decode_response(&[value]).unwrap(), Some(expected));
        }
    }

    #[test]
    fn a_response_we_do_not_know_decodes_to_none() {
        assert_eq!(decode_response(&[9]).unwrap(), None);
    }

    #[test]
    fn an_empty_response_body_fails_cleanly() {
        assert!(decode_response(&[]).is_err());
    }

    /// Four flags, a nil template, and an empty list: 4 + 16 + 1 + 2 bytes.
    #[test]
    fn packs_info_has_the_shape_the_schema_describes() {
        let packet = packs_info_empty();
        assert_eq!(packet.id, ID_RESOURCE_PACKS_INFO);
        assert_eq!(packet.body.len(), 4 + 16 + 1 + 2);
        assert!(
            packet.body[..4].iter().all(|&b| b == 0),
            "nothing is required or present"
        );
    }

    #[test]
    fn the_stack_carries_the_base_game_version() {
        let packet = pack_stack_empty("1.26.30");
        assert_eq!(packet.id, ID_RESOURCE_PACK_STACK);

        let mut r = Reader::new(&packet.body);
        assert_eq!(r.u8().unwrap(), 0, "not required");
        assert_eq!(r.varint().unwrap(), 0, "no packs");
        assert_eq!(r.prefixed().unwrap(), b"1.26.30");
    }

    /// The experiment count is a fixed u32 while the pack count above it is a varint.
    /// Writing a varint here leaves the packet three bytes short.
    #[test]
    fn the_stack_declares_no_experiments() {
        let packet = pack_stack_empty("*");
        let mut r = Reader::new(&packet.body);
        r.u8().unwrap();
        r.varint().unwrap();
        r.prefixed().unwrap();
        assert_eq!(r.u32().unwrap(), 0, "no toggles, as a fixed u32");
        assert_eq!(r.u8().unwrap(), 0, "never toggled");
        assert_eq!(r.u8().unwrap(), 0, "no editor packs");
        assert!(r.is_empty());
    }
}
