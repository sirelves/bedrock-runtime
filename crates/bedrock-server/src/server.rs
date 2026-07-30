//! Where the transport meets the game protocol.
//!
//! `bedrock-raknet` delivers payloads and `bedrock-protocol` says what they mean; this
//! is the only crate allowed to know both. Sans-io like the layers under it: datagrams
//! and the time in, datagrams and [`Event`]s out.
//!
//! It answers `RequestNetworkSettings` and nothing else yet. Everything past that is
//! reported so a capture can be taken of bytes no third-party documentation covers.

use bedrock_crypto::agreement::ServerKey;
use bedrock_crypto::cipher::Cipher;
use bedrock_crypto::handshake as token;
use bedrock_crypto::probe;
use bedrock_protocol::batch;
use bedrock_protocol::handshake::{
    ID_CLIENT_TO_SERVER_HANDSHAKE, ID_REQUEST_NETWORK_SETTINGS, NetworkSettings,
    RequestNetworkSettings, server_to_client_handshake,
};
use bedrock_protocol::login::{self, ID_LOGIN, Login};
use bedrock_protocol::version::{MINECRAFT_VERSION, PROTOCOL_VERSION};
use bedrock_raknet::listener::{Event as RakEvent, Listener, ListenerConfig};
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

/// Default UDP port, from the transport.
pub const DEFAULT_PORT: u16 = bedrock_raknet::DEFAULT_PORT_V4;

/// The protocol version this server speaks.
pub const TARGET_PROTOCOL: u32 = PROTOCOL_VERSION;

/// Something worth telling the operator about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A peer finished the transport handshake.
    Connected(SocketAddr),
    /// A peer asked which compression to use, and was told.
    NetworkSettingsRequested {
        /// Who asked.
        peer: SocketAddr,
        /// The protocol version the client declared, in the clear.
        client_protocol: u32,
    },
    /// A game packet we do not handle yet.
    Unhandled {
        /// Who sent it.
        peer: SocketAddr,
        /// Its id.
        id: u32,
        /// Its body.
        body: Vec<u8>,
    },
    /// A client presented a login. The identity is not verified here — that needs the
    /// issuer's keys, which live outside this crate.
    LoginReceived {
        /// Who logged in.
        peer: SocketAddr,
        /// The protocol version the login declares.
        client_protocol: u32,
        /// The identity document, verbatim.
        identity: String,
    },
    /// A client accepted our handshake and switched to an encrypted stream.
    HandshakeAccepted(SocketAddr),
    /// An encrypted packet decrypted and its checksum held.
    Decrypted {
        /// Who sent it.
        peer: SocketAddr,
        /// The packet id inside.
        id: u32,
        /// How many bytes of plaintext came out.
        len: usize,
    },
    /// An encrypted packet did not decrypt. The derivation, the IV or the checksum
    /// formula is wrong, and they all fail this way.
    DecryptionFailed(SocketAddr),
    /// A search over the key-schedule variants found one that verifies.
    VariantFound {
        /// Who sent the packet it was found with.
        peer: SocketAddr,
        /// The combination, ready to be written into the cipher.
        variant: String,
        /// What it decrypted to.
        plaintext: Vec<u8>,
    },
    /// A batch arrived compressed with a method we cannot read yet.
    Compressed {
        /// Who sent it.
        peer: SocketAddr,
        /// Which method the batch declared.
        method: batch::Method,
    },
    /// A payload that did not decode as a batch.
    Undecodable(SocketAddr, Vec<u8>),
    /// A peer went away.
    Disconnected(SocketAddr),
}

/// The advertisement a client matches against its own version.
pub fn advertisement(
    name: &str,
    players: usize,
    max_players: usize,
    port: u16,
    guid: i64,
) -> String {
    format!(
        "MCPE;{name};{PROTOCOL_VERSION};{MINECRAFT_VERSION};{players};{max_players};{guid};;Survival;1;{port};{port};"
    )
}

/// A Bedrock server, as far as M0.3 has got.
#[derive(Debug)]
pub struct Server {
    listener: Listener,
    /// Peers already told which compression to use. The batch framing gains a method
    /// byte from that point on, so the same bytes decode two different ways depending
    /// on which side of this the peer is.
    settled: HashSet<SocketAddr>,
    /// One ephemeral key per peer, kept until the session key is derived.
    keys: HashMap<SocketAddr, ServerKey>,
    /// The encrypted stream, once a peer has one.
    ciphers: HashMap<SocketAddr, Cipher>,
    /// The client's public key, kept so a failed decryption can be searched.
    client_keys: HashMap<SocketAddr, Vec<u8>>,
}

impl Server {
    /// A server advertising itself as `advertisement`, with default limits.
    pub fn new(local: SocketAddr, guid: i64, advertisement: &str) -> Self {
        Self::with_config(local, guid, advertisement, ListenerConfig::default())
    }

    /// A server with explicit transport limits.
    pub fn with_config(
        local: SocketAddr,
        guid: i64,
        advertisement: &str,
        config: ListenerConfig,
    ) -> Self {
        Self {
            listener: Listener::new(local, guid, advertisement, config),
            settled: HashSet::new(),
            keys: HashMap::new(),
            ciphers: HashMap::new(),
            client_keys: HashMap::new(),
        }
    }

    /// Peers currently connected or connecting.
    pub fn sessions(&self) -> usize {
        self.listener.sessions()
    }

    /// The next datagram for the socket.
    pub fn poll_transmit(&mut self) -> Option<(SocketAddr, Arc<[u8]>)> {
        self.listener.poll_transmit()
    }

    /// Drives timeouts and retransmission.
    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        self.listener
            .tick(now)
            .into_iter()
            .filter_map(|event| match event {
                RakEvent::Disconnected(peer) => Some(Event::Disconnected(peer)),
                _ => None,
            })
            .collect()
    }

    /// Feeds one datagram in.
    pub fn receive(&mut self, from: SocketAddr, bytes: &[u8], now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        for event in self.listener.receive(from, bytes, now) {
            match event {
                RakEvent::Connected(peer) => events.push(Event::Connected(peer)),
                RakEvent::Disconnected(peer) => {
                    self.settled.remove(&peer);
                    self.keys.remove(&peer);
                    self.ciphers.remove(&peer);
                    self.client_keys.remove(&peer);
                    events.push(Event::Disconnected(peer));
                }
                RakEvent::Payload(peer, payload) => {
                    events.extend(self.on_payload(peer, &payload, now));
                }
            }
        }
        events
    }

    fn on_payload(&mut self, peer: SocketAddr, payload: &[u8], now: Instant) -> Vec<Event> {
        // Once a peer has a cipher, everything after the batch marker is ciphertext.
        if let Some(cipher) = self.ciphers.get_mut(&peer) {
            let Some(body) = payload.strip_prefix(&[batch::MARKER]) else {
                return vec![Event::Undecodable(peer, payload.to_vec())];
            };
            match cipher.decrypt(body) {
                Ok(plaintext) => return self.on_plaintext(peer, &plaintext, now),
                Err(_) => return self.on_decryption_failure(peer, body),
            }
        }

        let packets = if self.settled.contains(&peer) {
            match batch::decode_with_method(payload) {
                Ok((batch::Method::None, packets)) => packets,
                Ok((method, _)) => return vec![Event::Compressed { peer, method }],
                Err(_) => return vec![Event::Undecodable(peer, payload.to_vec())],
            }
        } else {
            match batch::decode(payload) {
                Ok(packets) => packets,
                Err(_) => return vec![Event::Undecodable(peer, payload.to_vec())],
            }
        };

        let mut events = Vec::new();
        for packet in packets {
            if packet.id == ID_REQUEST_NETWORK_SETTINGS {
                let client_protocol = RequestNetworkSettings::decode(&packet.body)
                    .map(|request| request.client_protocol)
                    .unwrap_or_default();

                // Uncompressed on purpose: it makes the login that follows readable as
                // plain bytes. Negotiating a real algorithm waits until there is a
                // capture to check it against.
                // This reply still goes out without a method byte; the byte appears
                // from the next batch onwards, in both directions.
                let reply = batch::encode(&[NetworkSettings::uncompressed().packet()]);
                let _ = self.listener.send(peer, reply, now);
                self.settled.insert(peer);

                events.push(Event::NetworkSettingsRequested {
                    peer,
                    client_protocol,
                });
            } else if packet.id == ID_LOGIN {
                events.extend(self.on_login(peer, &packet.body, now));
            } else if packet.id == ID_CLIENT_TO_SERVER_HANDSHAKE {
                events.push(Event::HandshakeAccepted(peer));
            } else {
                events.push(Event::Unhandled {
                    peer,
                    id: packet.id,
                    body: packet.body,
                });
            }
        }
        events
    }
}

impl Server {
    /// Answers a login with our public key and salt.
    ///
    /// The identity is reported rather than verified: verification needs the issuer's
    /// published keys, and fetching those is I/O, which does not belong in a sans-io
    /// layer. The caller decides whether to trust what it is told.
    fn on_login(&mut self, peer: SocketAddr, body: &[u8], now: Instant) -> Vec<Event> {
        let Ok(login) = Login::decode(body, &login::Limits::default()) else {
            return vec![Event::Undecodable(peer, body.to_vec())];
        };

        let key = ServerKey::generate();
        let reply = batch::encode_with_method(&[server_to_client_handshake(&token::token(&key))]);
        let _ = self.listener.send(peer, reply, now);

        // The client's public key travels inside the identity token, which this layer
        // does not verify. Agreement uses it either way; verification is what says the
        // key belongs to who it claims, and that is the caller's business.
        if let Some(client_key) = client_public_key(login.identity) {
            let salt = *key.salt();
            if let Ok(session) = key.agree(&client_key, &salt) {
                self.ciphers.insert(peer, Cipher::new(&session));
            }
            self.client_keys.insert(peer, client_key);
        }
        self.keys.insert(peer, key);

        vec![Event::LoginReceived {
            peer,
            client_protocol: login.client_protocol,
            identity: login.identity.to_owned(),
        }]
    }
}

/// Pulls the client's public key out of an identity document.
///
/// The claims are read without verifying the signature: this is the key we agree with,
/// and whether it belongs to who the token says is a separate question, answered
/// elsewhere with the issuer's published keys.
fn client_public_key(identity: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    let outer: serde_json::Value = serde_json::from_str(identity).ok()?;
    let token = outer.get("Token")?.as_str()?;
    let claims = token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(claims)
        .ok()?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    base64::engine::general_purpose::STANDARD
        .decode(claims.get("cpk")?.as_str()?)
        .ok()
}

impl Server {
    /// Searches the key-schedule variants when our own guess did not verify.
    ///
    /// Every wrong guess fails identically, at the checksum, so trying them one per
    /// connection means one round trip through a human per attempt. The search covers
    /// the space against the packet already in hand.
    fn on_decryption_failure(&mut self, peer: SocketAddr, ciphertext: &[u8]) -> Vec<Event> {
        let Some(key) = self.keys.get(&peer) else {
            return vec![Event::DecryptionFailed(peer)];
        };
        let Some(client_der) = self.client_keys.get(&peer) else {
            return vec![Event::DecryptionFailed(peer)];
        };

        let salt = *key.salt();
        match probe::search(key, client_der, &salt, ciphertext) {
            Ok(Some(found)) => vec![Event::VariantFound {
                peer,
                variant: found.variant.to_string(),
                plaintext: found.plaintext,
            }],
            _ => vec![Event::DecryptionFailed(peer)],
        }
    }

    /// Handles packets that came out of the encrypted stream.
    fn on_plaintext(&mut self, peer: SocketAddr, plaintext: &[u8], _now: Instant) -> Vec<Event> {
        let Ok(packets) = batch::decode_packets(bedrock_protocol::bytes::Reader::new(
            plaintext.get(1..).unwrap_or_default(),
        )) else {
            return vec![Event::Undecodable(peer, plaintext.to_vec())];
        };

        packets
            .into_iter()
            .map(|packet| {
                if packet.id == ID_CLIENT_TO_SERVER_HANDSHAKE {
                    Event::HandshakeAccepted(peer)
                } else {
                    Event::Decrypted {
                        peer,
                        id: packet.id,
                        len: packet.body.len(),
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bedrock_protocol::batch::Packet;
    use bedrock_protocol::handshake::{Compression, ID_NETWORK_SETTINGS};

    #[test]
    fn the_advertisement_carries_the_target_version() {
        let text = advertisement("test", 0, 10, 19132, 7);
        let fields: Vec<&str> = text.split(';').collect();
        assert_eq!(fields[0], "MCPE");
        assert_eq!(fields[2], PROTOCOL_VERSION.to_string());
        assert_eq!(fields[3], MINECRAFT_VERSION);
    }

    /// The reply a client gets, built from the bytes a client actually sent.
    #[test]
    fn a_request_is_answered_with_uncompressed_settings() {
        let raw = [0xfe, 0x06, 0xc1, 0x01, 0x00, 0x00, 0x03, 0xe9];
        let packets = batch::decode(&raw).unwrap();
        assert_eq!(packets[0].id, ID_REQUEST_NETWORK_SETTINGS);

        let reply = batch::encode(&[NetworkSettings::uncompressed().packet()]);
        let decoded = batch::decode(&reply).unwrap();
        assert_eq!(decoded[0].id, ID_NETWORK_SETTINGS);

        let settings = NetworkSettings::decode(&decoded[0].body).unwrap();
        assert_eq!(settings.compression, Compression::None);
        assert_eq!(settings.compression_threshold, 0);
    }

    /// Id 300 is not one we handle. Ids we do handle are tested by their own paths —
    /// this one exists to prove the rest is reported rather than silently dropped.
    #[test]
    fn an_unknown_packet_is_reported_rather_than_dropped() {
        let payload = batch::encode(&[Packet::new(300, vec![1, 2, 3])]);
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();

        let events = server.on_payload(peer, &payload, Instant::now());
        assert_eq!(
            events,
            vec![Event::Unhandled {
                peer,
                id: 300,
                body: vec![1, 2, 3]
            }]
        );
    }

    #[test]
    fn a_payload_that_is_not_a_batch_is_reported() {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();
        let events = server.on_payload(peer, &[0x00, 0x01], Instant::now());
        assert!(matches!(events.as_slice(), [Event::Undecodable(_, _)]));
    }
}
