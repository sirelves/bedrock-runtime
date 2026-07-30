//! The packets that open a connection, before anything is encrypted.
//!
//! The client opens with `RequestNetworkSettings` and waits for `NetworkSettings`,
//! which decides the compression used from the next batch onwards. Layouts follow
//! Mojang's published schemas; the field order is theirs, and the fixtures under
//! `tests/fixtures/` are what proves we read it the same way a client writes it.

use crate::batch::Packet;
use crate::bytes::{DecodeError, Reader, Writer};

/// `RequestNetworkSettings`, client to server.
pub const ID_REQUEST_NETWORK_SETTINGS: u32 = 193;

/// `NetworkSettings`, server to client.
pub const ID_NETWORK_SETTINGS: u32 = 143;

/// How batches are compressed once negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compression {
    /// Raw deflate.
    ZLib = 0,
    /// Snappy.
    Snappy = 1,
    /// Nothing; packets follow the batch marker directly.
    None = 2,
}

impl Compression {
    /// Reads the wire value.
    pub fn from_value(value: u16) -> Option<Self> {
        Some(match value {
            0 => Self::ZLib,
            1 => Self::Snappy,
            2 => Self::None,
            _ => return None,
        })
    }
}

/// The client's opening packet: the protocol version it speaks, and nothing else.
///
/// Mojang's schema is explicit that nothing may be added here, which is what makes it
/// a stable place to read a version from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestNetworkSettings {
    /// The protocol version the client speaks.
    pub client_protocol: u32,
}

impl RequestNetworkSettings {
    /// Decodes the packet body.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        Ok(Self {
            // The one big-endian field, per Mojang's schema.
            client_protocol: Reader::new(body).u32_be()?,
        })
    }

    /// Encodes the packet body.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u32_be(self.client_protocol);
        w.finish()
    }
}

/// The server's answer, deciding compression and client-side throttling.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetworkSettings {
    /// Smallest payload worth compressing. Zero disables compression outright.
    pub compression_threshold: u16,
    /// Algorithm for every batch after this packet.
    pub compression: Compression,
    /// Whether the client should throttle how much it sends.
    pub client_throttle_enabled: bool,
    /// Player count above which the client throttles.
    pub client_throttle_threshold: u8,
    /// Scalar applied when throttling.
    pub client_throttle_scalar: f32,
}

impl NetworkSettings {
    /// Settings that turn compression off, both by algorithm and by threshold.
    ///
    /// Belt and braces on purpose: this is the configuration that lets a login be read
    /// as plain bytes, and a login that arrives compressed anyway is a finding, not a
    /// failure.
    pub fn uncompressed() -> Self {
        Self {
            compression_threshold: 0,
            compression: Compression::None,
            client_throttle_enabled: false,
            client_throttle_threshold: 0,
            client_throttle_scalar: 0.0,
        }
    }

    /// Encodes the packet body, in Mojang's ordinal order.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.u16(self.compression_threshold)
            .u16(self.compression as u16)
            .u8(u8::from(self.client_throttle_enabled))
            .u8(self.client_throttle_threshold)
            .f32(self.client_throttle_scalar);
        w.finish()
    }

    /// Decodes the packet body.
    pub fn decode(body: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(body);
        let compression_threshold = r.u16()?;
        let compression = Compression::from_value(r.u16()?).unwrap_or(Compression::None);
        Ok(Self {
            compression_threshold,
            compression,
            client_throttle_enabled: r.u8()? != 0,
            client_throttle_threshold: r.u8()?,
            client_throttle_scalar: r.f32()?,
        })
    }

    /// This packet, ready for a batch.
    pub fn packet(&self) -> Packet {
        Packet::new(ID_NETWORK_SETTINGS, self.encode())
    }
}

/// `ServerToClientHandshake`, server to client. Carries the token that starts encryption.
pub const ID_SERVER_TO_CLIENT_HANDSHAKE: u32 = 3;

/// `ClientToServerHandshake`, client to server. Empty; its arrival is the message.
pub const ID_CLIENT_TO_SERVER_HANDSHAKE: u32 = 4;

/// Wraps a signed handshake token in its packet.
///
/// The token is built by `bedrock-crypto`, which holds the key. This crate only knows
/// how a string goes on the wire.
pub fn server_to_client_handshake(token: &str) -> Packet {
    let mut w = Writer::new();
    w.prefixed(token.as_bytes());
    Packet::new(ID_SERVER_TO_CLIENT_HANDSHAKE, w.finish())
}

/// Reads the token out of a `ServerToClientHandshake` body.
pub fn decode_handshake_token(body: &[u8]) -> Result<&str, DecodeError> {
    let mut r = Reader::new(body);
    std::str::from_utf8(r.prefixed()?).map_err(|_| DecodeError::UnexpectedEnd {
        needed: 0,
        available: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_matches_the_captured_bytes() {
        let body = [0x00, 0x00, 0x03, 0xe9];
        assert_eq!(
            RequestNetworkSettings::decode(&body).unwrap(),
            RequestNetworkSettings {
                client_protocol: 1001
            }
        );
        assert_eq!(
            RequestNetworkSettings {
                client_protocol: 1001
            }
            .encode(),
            body
        );
    }

    #[test]
    fn network_settings_round_trips() {
        let settings = NetworkSettings {
            compression_threshold: 512,
            compression: Compression::Snappy,
            client_throttle_enabled: true,
            client_throttle_threshold: 20,
            client_throttle_scalar: 0.5,
        };
        assert_eq!(
            NetworkSettings::decode(&settings.encode()).unwrap(),
            settings
        );
    }

    /// Ten bytes, in Mojang's ordinal order: threshold, algorithm, enabled, threshold,
    /// scalar. A field out of order still decodes, silently and wrongly.
    #[test]
    fn the_body_is_ten_bytes_in_a_fixed_order() {
        let body = NetworkSettings::uncompressed().encode();
        assert_eq!(body.len(), 2 + 2 + 1 + 1 + 4);
        assert_eq!(body, vec![0, 0, 2, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn uncompressed_disables_it_twice_over() {
        let settings = NetworkSettings::uncompressed();
        assert_eq!(settings.compression, Compression::None);
        assert_eq!(settings.compression_threshold, 0);
    }

    #[test]
    fn an_unknown_algorithm_reads_as_none() {
        let mut w = Writer::new();
        w.u16(0).u16(99).u8(0).u8(0).f32(0.0);
        assert_eq!(
            NetworkSettings::decode(&w.finish()).unwrap().compression,
            Compression::None
        );
    }

    #[test]
    fn a_handshake_token_round_trips() {
        let token = "aaa.bbb.ccc";
        let packet = server_to_client_handshake(token);
        assert_eq!(packet.id, ID_SERVER_TO_CLIENT_HANDSHAKE);
        assert_eq!(decode_handshake_token(&packet.body).unwrap(), token);
    }

    /// A token is length-prefixed, not the whole body, so a batch can carry more after it.
    #[test]
    fn the_token_is_length_prefixed() {
        let packet = server_to_client_handshake("ab");
        assert_eq!(packet.body, vec![2, b'a', b'b']);
    }

    #[test]
    fn a_truncated_token_fails_cleanly() {
        assert!(decode_handshake_token(&[9, b'a']).is_err());
        assert!(decode_handshake_token(&[]).is_err());
    }

    #[test]
    fn truncated_bodies_fail_cleanly() {
        let full = NetworkSettings::uncompressed().encode();
        for n in 0..full.len() {
            assert!(NetworkSettings::decode(&full[..n]).is_err(), "{n}");
        }
        assert!(RequestNetworkSettings::decode(&[0, 0]).is_err());
    }
}
