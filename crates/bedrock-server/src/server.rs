//! Where the transport meets the game protocol.
//!
//! `bedrock-raknet` delivers payloads and `bedrock-protocol` says what they mean; this
//! is the only crate allowed to know both. Sans-io like the layers under it: datagrams
//! and the time in, datagrams and [`Event`]s out.
//!
//! It answers `RequestNetworkSettings` and nothing else yet. Everything past that is
//! reported so a capture can be taken of bytes no third-party documentation covers.

use base64::Engine;
use bedrock_crypto::agreement::ServerKey;
use bedrock_crypto::cipher::Cipher;
use bedrock_crypto::handshake as token;
use bedrock_crypto::jwt::{self, Expected};
use bedrock_protocol::batch;
use bedrock_protocol::handshake::{
    ID_CLIENT_TO_SERVER_HANDSHAKE, ID_REQUEST_NETWORK_SETTINGS, NetworkSettings,
    RequestNetworkSettings, server_to_client_handshake,
};
use bedrock_protocol::login::{self, ID_LOGIN, Login, TOKEN_AUDIENCE, TOKEN_ISSUER};
use bedrock_protocol::play_status::{self, Status};
use bedrock_protocol::resource_packs::{self, ID_RESOURCE_PACK_CLIENT_RESPONSE, Response};
use bedrock_protocol::start_game::StartGame;
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

/// Where the identity issuer publishes its signing keys.
pub const TOKEN_KEYS_URL: &str = bedrock_protocol::login::TOKEN_KEYS_URL;

pub use bedrock_crypto::jwt::Jwks;

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
    /// A client presented a login and its identity token verified.
    LoginAccepted {
        /// Who logged in.
        peer: SocketAddr,
        /// The protocol version the login declares.
        client_protocol: u32,
        /// The gamertag the issuer vouched for, when the token carried one.
        gamertag: Option<String>,
    },
    /// A login was refused because its identity did not hold up.
    LoginRejected {
        /// Who tried.
        peer: SocketAddr,
        /// Why, in the issuer's terms.
        reason: String,
    },
    /// A client accepted our handshake and switched to an encrypted stream.
    HandshakeAccepted(SocketAddr),
    /// The server told a client where its login stands.
    PlayStatusSent {
        /// Who it went to.
        peer: SocketAddr,
        /// What it said.
        status: Status,
    },
    /// The client answered the resource pack offer.
    PacksAnswered {
        /// Who answered.
        peer: SocketAddr,
        /// What it said.
        response: Response,
    },
    /// The client has finished with packs and was sent a world description.
    ReadyForWorld(SocketAddr),
    /// `StartGame` went out.
    WorldSent(SocketAddr),
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
    /// The protocol version each peer declared at login.
    protocols: HashMap<SocketAddr, u32>,
    /// What the world calls itself in the client.
    world_name: String,
    /// The issuer's published keys, and when they were set. Empty until the caller
    /// supplies them, and a login cannot be verified without them.
    identity_keys: Option<Jwks>,
    /// Wall-clock anchor: the caller reads the clock once, and everything after is
    /// measured from the monotonic instant it was read at.
    clock: Option<(i64, Instant)>,
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
            world_name: "bedrock-runtime".to_owned(),
            settled: HashSet::new(),
            keys: HashMap::new(),
            ciphers: HashMap::new(),
            protocols: HashMap::new(),
            identity_keys: None,
            clock: None,
        }
    }

    /// Supplies the issuer's signing keys, and anchors wall-clock time.
    ///
    /// Fetching these is I/O and belongs to the caller. Until they arrive no login can
    /// be verified, and an unverifiable login is refused rather than waved through:
    /// a server that silently stops checking identities when a fetch fails is worse
    /// than one that never checked.
    pub fn set_identity_keys(&mut self, keys: Jwks, unix_now: i64, now: Instant) {
        self.identity_keys = Some(keys);
        self.clock = Some((unix_now, now));
    }

    /// Whether logins can be verified at all.
    pub fn can_verify_identity(&self) -> bool {
        self.identity_keys.is_some() && self.clock.is_some()
    }

    fn unix_now(&self, now: Instant) -> Option<i64> {
        let (anchor_unix, anchor) = self.clock?;
        let elapsed = i64::try_from(now.saturating_duration_since(anchor).as_secs()).ok()?;
        Some(anchor_unix + elapsed)
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
                    self.protocols.remove(&peer);
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
            let Ok(plaintext) = cipher.decrypt(body) else {
                return vec![Event::DecryptionFailed(peer)];
            };
            return self.on_plaintext(peer, &plaintext, now);
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
    /// Verifies a login and, if it holds, answers with our public key and salt.
    ///
    /// A refused login is told so in the clear, before encryption starts: there is no
    /// encrypted channel to say it on yet, and the alternative is a client staring at a
    /// connecting screen with no explanation.
    fn on_login(&mut self, peer: SocketAddr, body: &[u8], now: Instant) -> Vec<Event> {
        let Ok(login) = Login::decode(body, &login::Limits::default()) else {
            return vec![Event::Undecodable(peer, body.to_vec())];
        };

        let claims = match self.verify_identity(login.identity, now) {
            Ok(claims) => claims,
            Err(reason) => {
                let refusal =
                    batch::encode_with_method(&[play_status::packet(Status::InvalidTenant)]);
                let _ = self.listener.send(peer, refusal, now);
                return vec![Event::LoginRejected { peer, reason }];
            }
        };

        let key = ServerKey::generate();
        let reply = batch::encode_with_method(&[server_to_client_handshake(&token::token(&key))]);
        let _ = self.listener.send(peer, reply, now);

        // The key to agree with is the one the issuer signed for, not whatever the
        // login also happens to carry: that is the whole point of having verified it.
        if let Some(client_key) = claims
            .cpk
            .as_deref()
            .and_then(|cpk| base64::engine::general_purpose::STANDARD.decode(cpk).ok())
        {
            let salt = *key.salt();
            if let Ok(session) = key.agree(&client_key, &salt) {
                self.ciphers.insert(peer, Cipher::new(&session));
            }
        }
        self.keys.insert(peer, key);
        self.protocols.insert(peer, login.client_protocol);

        vec![Event::LoginAccepted {
            peer,
            client_protocol: login.client_protocol,
            gamertag: claims.xname,
        }]
    }

    /// Checks the identity token against the issuer's published keys.
    fn verify_identity(&self, identity: &str, now: Instant) -> Result<jwt::Claims, String> {
        let Some(keys) = &self.identity_keys else {
            return Err("server has no issuer keys, so no login can be verified".to_owned());
        };
        let Some(unix_now) = self.unix_now(now) else {
            return Err("server has no wall-clock anchor".to_owned());
        };

        let outer: serde_json::Value =
            serde_json::from_str(identity).map_err(|_| "identity is not JSON".to_owned())?;
        let token = outer
            .get("Token")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "identity has no Token".to_owned())?;

        let expected = Expected {
            issuer: TOKEN_ISSUER.to_owned(),
            audience: TOKEN_AUDIENCE.to_owned(),
            leeway: 60,
        };
        jwt::verify(token, keys, &expected, unix_now).map_err(|e| e.to_string())
    }
}

impl Server {
    /// Sends a batch through the peer's encrypted stream.
    ///
    /// The batch marker stays in the clear — it is how the receiver recognises the
    /// payload at all — and everything after it is encrypted.
    fn send_encrypted(&mut self, peer: SocketAddr, packets: &[batch::Packet], now: Instant) {
        let framed = batch::encode_with_method(packets);
        let Some(body) = framed.strip_prefix(&[batch::MARKER]) else {
            return;
        };
        let Some(cipher) = self.ciphers.get_mut(&peer) else {
            return;
        };

        let mut payload = vec![batch::MARKER];
        payload.extend_from_slice(&cipher.encrypt(body));
        let _ = self.listener.send(peer, payload, now);
    }

    /// Handles packets that came out of the encrypted stream.
    fn on_plaintext(&mut self, peer: SocketAddr, plaintext: &[u8], now: Instant) -> Vec<Event> {
        let Ok(packets) = batch::decode_packets(bedrock_protocol::bytes::Reader::new(
            plaintext.get(1..).unwrap_or_default(),
        )) else {
            return vec![Event::Undecodable(peer, plaintext.to_vec())];
        };

        let mut events = Vec::new();
        for packet in packets {
            if packet.id == ID_CLIENT_TO_SERVER_HANDSHAKE {
                events.push(Event::HandshakeAccepted(peer));

                // The verdict names the side that is behind, so a mismatched player is
                // told to update the right thing instead of watching a blank screen.
                let declared = self
                    .protocols
                    .get(&peer)
                    .copied()
                    .unwrap_or(TARGET_PROTOCOL);
                let status = Status::for_version_mismatch(declared, TARGET_PROTOCOL)
                    .unwrap_or(Status::LoginSuccess);

                self.send_encrypted(peer, &[play_status::packet(status)], now);
                events.push(Event::PlayStatusSent { peer, status });

                if status == Status::LoginSuccess {
                    self.send_encrypted(peer, &[resource_packs::packs_info_empty()], now);
                }
            } else if packet.id == ID_RESOURCE_PACK_CLIENT_RESPONSE {
                let response = resource_packs::decode_response(&packet.body).ok().flatten();
                if let Some(response) = response {
                    events.push(Event::PacksAnswered { peer, response });

                    // Observed against a real client: with an empty offer it answers
                    // StackFinished straight away rather than walking the two-step
                    // flow, so waiting only for DownloadingFinished leaves it hanging.
                    match response {
                        Response::DownloadingFinished => {
                            let stack = resource_packs::pack_stack_empty(MINECRAFT_VERSION);
                            self.send_encrypted(peer, &[stack], now);
                        }
                        Response::StackFinished => {
                            events.push(Event::ReadyForWorld(peer));

                            let world = StartGame::flat(
                                &self.world_name,
                                MINECRAFT_VERSION,
                                concat!("bedrock-runtime ", env!("CARGO_PKG_VERSION")),
                            );
                            self.send_encrypted(peer, &[world.packet()], now);
                            events.push(Event::WorldSent(peer));
                        }
                        _ => {}
                    }
                }
            } else {
                events.push(Event::Decrypted {
                    peer,
                    id: packet.id,
                    len: packet.body.len(),
                });
            }
        }
        events
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

    fn a_login(identity: &str) -> Vec<u8> {
        use bedrock_protocol::bytes::Writer;
        let mut blob = Writer::new();
        blob.u32(u32::try_from(identity.len()).unwrap_or(0))
            .bytes(identity.as_bytes())
            .u32(0);
        let mut w = Writer::new();
        w.u32_be(TARGET_PROTOCOL).prefixed(&blob.finish());
        batch::encode(&[Packet::new(ID_LOGIN, w.finish())])
    }

    /// The property the whole milestone rests on: a server that cannot verify refuses.
    /// Falling back to accepting whoever asks, when a key fetch fails, is worse than
    /// never having checked — it looks authenticated and is not.
    #[test]
    fn without_issuer_keys_a_login_is_refused() {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();
        assert!(!server.can_verify_identity());

        let payload = a_login(r#"{"Token":"aaa.bbb.ccc"}"#);
        let events = server.on_payload(peer, &payload, Instant::now());

        assert!(
            matches!(events.as_slice(), [Event::LoginRejected { .. }]),
            "{events:?}"
        );
    }

    /// A refused login must not leave a cipher behind: the peer never agreed on a key,
    /// and treating it as encrypted would turn every later packet into a decode failure.
    #[test]
    fn a_refused_login_starts_no_encryption() {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();
        server.on_payload(peer, &a_login(r#"{"Token":"aaa.bbb.ccc"}"#), Instant::now());
        assert!(!server.ciphers.contains_key(&peer));
    }

    #[test]
    fn a_login_that_is_not_json_is_refused_not_crashed() {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();
        for identity in ["", "not json", "{}", r#"{"Token":42}"#] {
            let events = server.on_payload(peer, &a_login(identity), Instant::now());
            assert!(
                matches!(events.as_slice(), [Event::LoginRejected { .. }]),
                "{identity:?} -> {events:?}"
            );
        }
    }

    /// Keys alone are not enough: without a clock anchor an expiry cannot be judged,
    /// and judging it against nothing would accept expired tokens forever.
    #[test]
    fn keys_and_a_clock_are_both_required() {
        let server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        assert!(!server.can_verify_identity());

        let mut with_keys = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let keys = Jwks::parse(r#"{"keys":[]}"#).unwrap();
        with_keys.set_identity_keys(keys, 1_700_000_000, Instant::now());
        assert!(with_keys.can_verify_identity());
    }

    #[test]
    fn a_payload_that_is_not_a_batch_is_reported() {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();
        let events = server.on_payload(peer, &[0x00, 0x01], Instant::now());
        assert!(matches!(events.as_slice(), [Event::Undecodable(_, _)]));
    }
}
