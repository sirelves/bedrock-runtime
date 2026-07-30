//! View distance: what the client asks for, and what the server grants.
//!
//! The client sends its request as soon as it accepts a world, and then waits. It is
//! the first thing it asks for after `StartGame`, and leaving it unanswered leaves the
//! client on a loading screen with nothing to load.

use crate::batch::Packet;
use crate::bytes::{DecodeError, Reader, Writer};

/// `RequestChunkRadius`, client to server.
pub const ID_REQUEST_CHUNK_RADIUS: u32 = 69;

/// `ChunkRadiusUpdated`, server to client.
pub const ID_CHUNK_RADIUS_UPDATED: u32 = 70;

/// `ServerboundLoadingScreen`, client to server. Announces that a loading screen
/// opened or closed; nothing is expected back.
pub const ID_SERVERBOUND_LOADING_SCREEN: u32 = 312;

/// What the client asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    /// The radius it wants, in chunks.
    pub radius: i32,
    /// The largest it is willing to accept.
    pub max_radius: u8,
}

/// Decodes a `RequestChunkRadius` body.
pub fn decode_request(body: &[u8]) -> Result<Request, DecodeError> {
    let mut r = Reader::new(body);
    Ok(Request {
        radius: r.zigzag32()?,
        max_radius: r.u8()?,
    })
}

/// Builds `ChunkRadiusUpdated`.
///
/// The granted radius is the server's decision, not a confirmation: a client asking
/// for more than the server will stream has to be told the smaller number, or it waits
/// for chunks that are never coming.
pub fn granted(radius: i32) -> Packet {
    let mut w = Writer::new();
    w.zigzag32(radius);
    Packet::new(ID_CHUNK_RADIUS_UPDATED, w.finish())
}

/// The radius to grant, given what the client asked and what the server serves.
pub fn grant(request: &Request, server_max: i32) -> i32 {
    request.radius.clamp(1, server_max.max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_decodes() {
        let mut w = Writer::new();
        w.zigzag32(12).u8(16);
        assert_eq!(
            decode_request(&w.finish()).unwrap(),
            Request {
                radius: 12,
                max_radius: 16
            }
        );
    }

    #[test]
    fn a_truncated_request_fails_cleanly() {
        let mut w = Writer::new();
        w.zigzag32(12);
        assert!(decode_request(&w.finish()).is_err());
        assert!(decode_request(&[]).is_err());
    }

    /// Granting more than the server streams leaves the client waiting for chunks that
    /// never arrive, so the answer is the smaller of the two.
    #[test]
    fn the_grant_never_exceeds_what_the_server_serves() {
        let asked = Request {
            radius: 32,
            max_radius: 64,
        };
        assert_eq!(grant(&asked, 4), 4);
    }

    #[test]
    fn a_modest_request_is_granted_as_asked() {
        let asked = Request {
            radius: 2,
            max_radius: 16,
        };
        assert_eq!(grant(&asked, 8), 2);
    }

    /// A radius of zero would mean no chunks at all, which is not a world.
    #[test]
    fn the_grant_is_never_zero_or_negative() {
        for radius in [0, -1, i32::MIN] {
            let asked = Request {
                radius,
                max_radius: 16,
            };
            assert_eq!(grant(&asked, 8), 1, "{radius}");
        }
        assert_eq!(
            grant(
                &Request {
                    radius: 4,
                    max_radius: 16
                },
                0
            ),
            1
        );
    }

    #[test]
    fn the_grant_round_trips_through_the_packet() {
        let packet = granted(6);
        assert_eq!(packet.id, ID_CHUNK_RADIUS_UPDATED);
        assert_eq!(Reader::new(&packet.body).zigzag32().unwrap(), 6);
    }
}
