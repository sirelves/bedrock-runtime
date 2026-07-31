//! What the client reports about the player once it is in the world.
//!
//! Both packets here are read-only: the client narrates, the server listens. Nothing is
//! expected back, which is why the world worked while they were being discarded.

use crate::bytes::{DecodeError, Reader};

/// `PlayerAuthInput`, client to server. Sent every tick once the player exists.
pub const ID_PLAYER_AUTH_INPUT: u32 = 144;

/// `SetLocalPlayerAsInitialized`, client to server.
///
/// The client sends this once it has finished loading and the player is actually
/// standing in the world. It is the client's own statement that the chunks arrived and
/// rendered, which makes it the only server-side proof that a world was accepted — a
/// client that silently discards every column never sends it.
pub const ID_SET_LOCAL_PLAYER_AS_INITIALIZED: u32 = 113;

/// Where the client says the player is and where they are looking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AuthInput {
    /// Where the head is pointing, up and down.
    pub pitch: f32,
    /// Where the body is facing.
    pub yaw: f32,
    /// Position along X.
    pub x: f32,
    /// Position along Y. This is the feet, not the eyes.
    pub y: f32,
    /// Position along Z.
    pub z: f32,
}

/// Decodes the head of a `PlayerAuthInput` body.
///
/// Only the first five floats are read. The rest of the packet is a bitset whose width
/// tracks the protocol version, followed by fields that appear conditionally on it —
/// none of which the server needs to know where the player is standing.
pub fn decode_auth_input(body: &[u8]) -> Result<AuthInput, DecodeError> {
    let mut r = Reader::new(body);
    Ok(AuthInput {
        pitch: r.f32()?,
        yaw: r.f32()?,
        x: r.f32()?,
        y: r.f32()?,
        z: r.f32()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Writer;

    #[test]
    fn the_position_follows_the_two_angles() {
        let mut w = Writer::new();
        w.f32(12.5) // pitch
            .f32(-90.0) // yaw
            .f32(1.5)
            .f32(80.0)
            .f32(-3.25)
            .f32(0.0) // moveVecX, and everything after, is ignored
            .f32(0.0);

        let input = decode_auth_input(&w.finish()).unwrap();
        assert_eq!(input.pitch, 12.5);
        assert_eq!(input.yaw, -90.0);
        assert_eq!((input.x, input.y, input.z), (1.5, 80.0, -3.25));
    }

    #[test]
    fn a_body_too_short_to_hold_a_position_is_rejected() {
        let mut w = Writer::new();
        w.f32(0.0).f32(0.0).f32(0.0);
        assert!(decode_auth_input(&w.finish()).is_err());
    }
}
