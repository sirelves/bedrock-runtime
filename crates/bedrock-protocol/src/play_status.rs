//! `PlayStatus`: the server's verdict on a login, and later the spawn signal.
//!
//! One `int32` big-endian. It is the first thing a client can be *told*, so it is also
//! the only way to fail a login visibly: without it a mismatched client sits on the
//! connecting screen until it times out, with nothing to show the player.

use crate::batch::Packet;
use crate::bytes::{DecodeError, Reader, Writer};

/// `PlayStatus`, server to client.
pub const ID_PLAY_STATUS: u32 = 2;

/// What the server is telling the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The login was accepted.
    LoginSuccess = 0,
    /// The client is older than this server speaks.
    ClientOutdated = 1,
    /// This server is older than the client speaks.
    ServerOutdated = 2,
    /// The world is ready and the player may spawn.
    PlayerSpawn = 3,
    /// The client's account is not valid for this server.
    InvalidTenant = 4,
    /// An Education client reached a vanilla server.
    EducationToVanilla = 5,
    /// A vanilla client reached an Education server.
    VanillaToEducation = 6,
    /// No room for another split-screen player.
    ServerFullForSubClient = 7,
}

impl Status {
    /// Reads the wire value.
    pub fn from_value(value: i32) -> Option<Self> {
        Some(match value {
            0 => Self::LoginSuccess,
            1 => Self::ClientOutdated,
            2 => Self::ServerOutdated,
            3 => Self::PlayerSpawn,
            4 => Self::InvalidTenant,
            5 => Self::EducationToVanilla,
            6 => Self::VanillaToEducation,
            7 => Self::ServerFullForSubClient,
            _ => return None,
        })
    }

    /// Which side is behind, given the version a client declared.
    ///
    /// Returns `None` when the versions match. Telling a client the wrong direction
    /// sends the player to update software that is already current.
    pub fn for_version_mismatch(client: u32, server: u32) -> Option<Self> {
        match client.cmp(&server) {
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Less => Some(Self::ClientOutdated),
            std::cmp::Ordering::Greater => Some(Self::ServerOutdated),
        }
    }
}

/// Encodes the packet body.
pub fn encode(status: Status) -> Vec<u8> {
    let mut w = Writer::new();
    w.u32_be(status as u32);
    w.finish()
}

/// Decodes the packet body.
pub fn decode(body: &[u8]) -> Result<Option<Status>, DecodeError> {
    let value = Reader::new(body).u32_be()? as i32;
    Ok(Status::from_value(value))
}

/// This status, ready for a batch.
pub fn packet(status: Status) -> Packet {
    Packet::new(ID_PLAY_STATUS, encode(status))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Status; 8] = [
        Status::LoginSuccess,
        Status::ClientOutdated,
        Status::ServerOutdated,
        Status::PlayerSpawn,
        Status::InvalidTenant,
        Status::EducationToVanilla,
        Status::VanillaToEducation,
        Status::ServerFullForSubClient,
    ];

    #[test]
    fn every_status_round_trips() {
        for status in ALL {
            assert_eq!(decode(&encode(status)).unwrap(), Some(status), "{status:?}");
        }
    }

    /// Big-endian, like the protocol version in the login and unlike most of the rest.
    #[test]
    fn the_body_is_a_big_endian_int32() {
        assert_eq!(encode(Status::PlayerSpawn), vec![0x00, 0x00, 0x00, 0x03]);
    }

    #[test]
    fn a_status_we_do_not_know_decodes_to_none() {
        let mut w = Writer::new();
        w.u32_be(99);
        assert_eq!(decode(&w.finish()).unwrap(), None);
    }

    /// Telling a client the wrong direction sends the player to update software that
    /// is already current.
    #[test]
    fn a_mismatch_names_the_side_that_is_behind() {
        assert_eq!(Status::for_version_mismatch(1001, 1001), None);
        assert_eq!(
            Status::for_version_mismatch(975, 1001),
            Some(Status::ClientOutdated)
        );
        assert_eq!(
            Status::for_version_mismatch(1002, 1001),
            Some(Status::ServerOutdated)
        );
    }

    #[test]
    fn the_packet_carries_the_right_id() {
        assert_eq!(packet(Status::LoginSuccess).id, ID_PLAY_STATUS);
    }

    #[test]
    fn a_truncated_body_fails_cleanly() {
        assert!(decode(&[0, 0]).is_err());
        assert!(decode(&[]).is_err());
    }
}
