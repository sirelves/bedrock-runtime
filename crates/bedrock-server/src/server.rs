//! Where the transport meets the game protocol.
//!
//! `bedrock-raknet` delivers payloads and `bedrock-protocol` says what they mean; this
//! is the only crate allowed to know both. Sans-io like the layers under it: datagrams
//! and the time in, datagrams and [`Event`]s out.
//!
//! It answers `RequestNetworkSettings` and nothing else yet. Everything past that is
//! reported so a capture can be taken of bytes no third-party documentation covers.

use crate::columns;
use base64::Engine;
use bedrock_crypto::agreement::ServerKey;
use bedrock_crypto::cipher::Cipher;
use bedrock_crypto::handshake as token;
use bedrock_crypto::jwt::{self, Expected};
use bedrock_protocol::batch;
use bedrock_protocol::chunk_radius::{
    self, ID_REQUEST_CHUNK_RADIUS, ID_SERVERBOUND_LOADING_SCREEN,
};
use bedrock_protocol::handshake::{
    ID_CLIENT_TO_SERVER_HANDSHAKE, ID_REQUEST_NETWORK_SETTINGS, NetworkSettings,
    RequestNetworkSettings, server_to_client_handshake,
};
use bedrock_protocol::level_chunk::{self, BIOME_PLAINS};
use bedrock_protocol::login::{self, ID_LOGIN, Login, TOKEN_AUDIENCE, TOKEN_ISSUER};
use bedrock_protocol::play_status::{self, Status};
use bedrock_protocol::player::{self, ID_PLAYER_AUTH_INPUT, ID_SET_LOCAL_PLAYER_AS_INITIALIZED};
use bedrock_protocol::registries;
use bedrock_protocol::resource_packs::{self, ID_RESOURCE_PACK_CLIENT_RESPONSE, Response};
use bedrock_protocol::start_game::StartGame;
use bedrock_protocol::version::{MINECRAFT_VERSION, PROTOCOL_VERSION};
use bedrock_raknet::listener::{Event as RakEvent, Listener, ListenerConfig};
pub use bedrock_raknet::session::Closed;
use bedrock_world::World;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;

/// Default UDP port, from the transport.
pub const DEFAULT_PORT: u16 = bedrock_raknet::DEFAULT_PORT_V4;

/// How far the login sequence is allowed to run.
///
/// A client that closes on a malformed packet says nothing about which packet. Stopping
/// the sequence early and seeing whether it still closes splits the suspects in half,
/// and costs one connection instead of one guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    /// Stop after `StartGame`, sending no registries.
    StartGame,
    /// Stop after the registries.
    World,
    /// Stop after granting a chunk radius.
    Radius,
    /// Stop after streaming columns, without telling the client to spawn.
    Chunks,
    /// Run the whole sequence.
    Spawn,
}

impl Stage {
    /// Parses a stage name.
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "start-game" => Self::StartGame,
            "world" => Self::World,
            "radius" => Self::Radius,
            "chunks" => Self::Chunks,
            "spawn" => Self::Spawn,
            _ => return None,
        })
    }
}

/// Where the flat world's surface sits. The player spawns standing on it.
pub const SURFACE_HEIGHT: i32 = 80;

/// How far this server streams chunks, in chunks.
///
/// Small on purpose while there is no world to stream: granting a radius the server
/// cannot fill leaves the client waiting for chunks that are not coming.
pub const SERVER_CHUNK_RADIUS: i32 = 4;

/// How far past the view distance a column is remembered as already sent.
///
/// Without a margin, a player standing on a chunk boundary crosses it back and forth
/// and the columns behind them are forgotten and re-sent at every step. Two chunks of
/// slack costs a little memory per player and turns that into nothing at all.
pub const FORGET_MARGIN: i32 = 2;

/// The protocol version this server speaks.
pub const TARGET_PROTOCOL: u32 = PROTOCOL_VERSION;

/// Where the identity issuer publishes its signing keys.
pub const TOKEN_KEYS_URL: &str = bedrock_protocol::login::TOKEN_KEYS_URL;

pub use bedrock_crypto::jwt::Jwks;

/// Something worth telling the operator about.
///
/// Not `Eq`: a reported position is a float, and one carried straight from the wire has
/// no meaningful notion of exact equality.
#[derive(Debug, Clone, PartialEq)]
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
    /// The world was streamed and the player told it may spawn.
    Spawned {
        /// Who spawned.
        peer: SocketAddr,
        /// How many columns went out.
        columns: usize,
    },
    /// The client asked for a view distance and was answered.
    ChunkRadiusGranted {
        /// Who asked.
        peer: SocketAddr,
        /// What it asked for.
        requested: i32,
        /// What it got.
        granted: i32,
    },
    /// The client reported the player standing in the world.
    ///
    /// This is the client's own statement that the chunks arrived and rendered. A client
    /// that discards every column it is sent never reaches this point.
    PlayerInitialized(SocketAddr),
    /// The player crossed into a new column and the world around it went out.
    ChunksStreamed {
        /// Who moved.
        peer: SocketAddr,
        /// The column they are standing in now, along X.
        chunk_x: i32,
        /// Along Z.
        chunk_z: i32,
        /// How many columns this move needed. Zero means they walked back over ground
        /// they had already been sent.
        columns: usize,
    },
    /// The client reported where the player is.
    PlayerMoved {
        /// Who moved.
        peer: SocketAddr,
        /// Where they say they are, along X.
        x: f32,
        /// Along Y, 1.62 above the feet — see `bedrock_protocol::player::POSITION_OFFSET`.
        y: f32,
        /// Along Z.
        z: f32,
    },
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
    /// A peer went away, and why.
    Disconnected(SocketAddr, Closed),
}

/// What the server knows about a player who is in the world.
///
/// Only what streaming needs: where they are, how far they see, and which columns they
/// have already been sent. Anything else about a player belongs to whatever milestone
/// introduces it.
#[derive(Debug)]
struct Player {
    /// View distance granted to this client, in chunks.
    radius: i32,
    /// The column the streaming is currently centred on.
    center: (i32, i32),
    /// Columns already sent. Sending one twice is bytes for nothing.
    sent: HashSet<(i32, i32)>,
    /// The last position the client reported, in blocks.
    position: (f32, f32, f32),
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
    /// Players in the world, and what they have been sent.
    players: HashMap<SocketAddr, Player>,
    /// The world itself. Generated in memory; no disk in M0 (ADR-011).
    world: World,
    /// What the world calls itself in the client.
    world_name: String,
    /// How far the login sequence runs before stopping.
    stage: Stage,
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
            stage: Stage::Spawn,
            settled: HashSet::new(),
            keys: HashMap::new(),
            ciphers: HashMap::new(),
            protocols: HashMap::new(),
            players: HashMap::new(),
            world: World::flat(SURFACE_HEIGHT),
            identity_keys: None,
            clock: None,
        }
    }

    /// Columns held in memory right now.
    pub fn loaded_columns(&self) -> usize {
        self.world.loaded()
    }

    /// Supplies the issuer's signing keys.
    ///
    /// Fetching these is I/O and belongs to the caller. Until they arrive no login can
    /// be verified, and an unverifiable login is refused rather than waved through:
    /// a server that silently stops checking identities when a fetch fails is worse
    /// than one that never checked.
    pub fn set_identity_keys(&mut self, keys: Jwks, unix_now: i64, now: Instant) {
        self.identity_keys = Some(keys);
        self.set_clock(unix_now, now);
    }

    /// Tells the server what time it is, in seconds since the epoch.
    ///
    /// Must be called regularly, not once. An earlier version anchored the wall clock
    /// at startup and derived the rest from [`Instant`], which does not count time the
    /// machine spent asleep — a laptop that suspends for ten minutes comes back
    /// believing it is ten minutes earlier, and starts refusing valid tokens as
    /// "issued in the future". Observed exactly that: 558 seconds of drift after a
    /// suspend, against a 60-second leeway.
    pub fn set_clock(&mut self, unix_now: i64, now: Instant) {
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

    /// Stops the login sequence early, for bisecting a client that closes on a packet.
    pub fn set_stage(&mut self, stage: Stage) {
        self.stage = stage;
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
                RakEvent::Disconnected(peer, reason) => Some(Event::Disconnected(peer, reason)),
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
                RakEvent::Disconnected(peer, reason) => {
                    self.settled.remove(&peer);
                    self.keys.remove(&peer);
                    self.ciphers.remove(&peer);
                    self.protocols.remove(&peer);
                    self.players.remove(&peer);
                    events.push(Event::Disconnected(peer, reason));
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
    /// Sends the world around spawn, then tells the client it may appear in it.
    ///
    /// `PlayStatus` goes last, after the columns. Telling a client to spawn into a
    /// world it has not received is how it ends up standing in nothing.
    fn stream_world(&mut self, peer: SocketAddr, radius: i32, now: Instant) -> Event {
        self.players.insert(
            peer,
            Player {
                radius: radius.max(0),
                center: (0, 0),
                sent: HashSet::new(),
                position: (0.0, SURFACE_HEIGHT as f32, 0.0),
            },
        );

        let columns = self.stream_around(peer, (0, 0), now);

        if self.stage >= Stage::Spawn {
            self.send_encrypted(peer, &[play_status::packet(Status::PlayerSpawn)], now);
        }

        Event::Spawned { peer, columns }
    }

    /// Sends whatever columns a player can see from `center` and has not been sent yet.
    ///
    /// The publisher update goes first: it names the point the client accepts columns
    /// around, and without it the columns are discarded however many are sent. It is
    /// re-sent every time the centre moves, which is what lets a walking player keep
    /// receiving world instead of walking off the edge of what spawn covered.
    fn stream_around(&mut self, peer: SocketAddr, center: (i32, i32), now: Instant) -> usize {
        let Some(player) = self.players.get_mut(&peer) else {
            return 0;
        };
        let radius = player.radius;
        let feet = (player.position.1 - player::POSITION_OFFSET) as i32;
        player.center = center;

        // Columns far behind are forgotten so that walking back into them sends them
        // again. The margin is what stops a player pacing across one boundary from
        // re-sending the same column every step.
        let forget = radius + FORGET_MARGIN;
        player
            .sent
            .retain(|&(x, z)| (x - center.0).abs() <= forget && (z - center.1).abs() <= forget);

        self.send_encrypted(
            peer,
            &[level_chunk::publisher_update(
                center.0 * level_chunk::CHUNK_WIDTH,
                feet,
                center.1 * level_chunk::CHUNK_WIDTH,
                (radius * level_chunk::CHUNK_WIDTH) as u32,
            )],
            now,
        );

        let wanted = level_chunk::columns_around(center.0, center.1, radius);
        let mut sent = 0;
        for (x, z) in wanted {
            let Some(player) = self.players.get_mut(&peer) else {
                break;
            };
            if !player.sent.insert((x, z)) {
                continue;
            }
            let column = columns::column_packet(self.world.chunk(x, z), BIOME_PLAINS);
            self.send_encrypted(peer, &[column], now);
            sent += 1;
        }
        sent
    }

    /// Takes a reported position and streams around it if the player crossed into a new
    /// column.
    fn on_player_moved(
        &mut self,
        peer: SocketAddr,
        input: player::AuthInput,
        now: Instant,
    ) -> Vec<Event> {
        let Some(player) = self.players.get_mut(&peer) else {
            return vec![Event::PlayerMoved {
                peer,
                x: input.x,
                y: input.y,
                z: input.z,
            }];
        };
        player.position = (input.x, input.y, input.z);

        let center = (
            (input.x.floor() as i32).div_euclid(level_chunk::CHUNK_WIDTH),
            (input.z.floor() as i32).div_euclid(level_chunk::CHUNK_WIDTH),
        );
        let moved = center != player.center;

        let mut events = vec![Event::PlayerMoved {
            peer,
            x: input.x,
            y: input.y,
            z: input.z,
        }];
        if moved {
            let columns = self.stream_around(peer, center, now);
            events.push(Event::ChunksStreamed {
                peer,
                chunk_x: center.0,
                chunk_z: center.1,
                columns,
            });
        }
        events
    }

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
                        Response::HaveAllPacks => {
                            let stack = resource_packs::pack_stack_empty(MINECRAFT_VERSION);
                            self.send_encrypted(peer, &[stack], now);
                        }
                        Response::Completed => {
                            events.push(Event::ReadyForWorld(peer));

                            let world = StartGame::flat(
                                &self.world_name,
                                MINECRAFT_VERSION,
                                concat!("bedrock-runtime ", env!("CARGO_PKG_VERSION")),
                                SURFACE_HEIGHT,
                            );
                            self.send_encrypted(peer, &[world.packet()], now);

                            // The registries are deliberately not sent. Bisection
                            // showed a client waits patiently after StartGame alone and
                            // disconnects as soon as the three empty registries arrive:
                            // an empty item, entity or biome list reads as "the server
                            // declares none" rather than "the server overrides none".
                            //
                            // Filling them needs data dumped from the game, which
                            // ADR-015 chose not to do. They were added to fix a crash
                            // that turned out to be the resource pack sequence, and the
                            // guess outlived the problem.
                            //
                            // `--stop-after world` still sends them, so the day real
                            // definitions exist the flag is the way to try them.
                            if self.stage == Stage::World {
                                for packet in registries::all_empty() {
                                    self.send_encrypted(peer, &[packet], now);
                                }
                            }

                            events.push(Event::WorldSent(peer));
                        }
                        _ => {}
                    }
                }
            } else if packet.id == ID_REQUEST_CHUNK_RADIUS {
                if let Ok(request) = chunk_radius::decode_request(&packet.body) {
                    if self.stage < Stage::Radius {
                        continue;
                    }
                    let granted = chunk_radius::grant(&request, SERVER_CHUNK_RADIUS);
                    self.send_encrypted(peer, &[chunk_radius::granted(granted)], now);
                    events.push(Event::ChunkRadiusGranted {
                        peer,
                        requested: request.radius,
                        granted,
                    });
                    if self.stage >= Stage::Chunks {
                        events.push(self.stream_world(peer, granted, now));
                    }
                }
            } else if packet.id == ID_SERVERBOUND_LOADING_SCREEN {
                // The client narrating its own loading screen. Nothing is expected back.
            } else if packet.id == ID_SET_LOCAL_PLAYER_AS_INITIALIZED {
                events.push(Event::PlayerInitialized(peer));
            } else if packet.id == ID_PLAYER_AUTH_INPUT {
                if let Ok(input) = player::decode_auth_input(&packet.body) {
                    events.extend(self.on_player_moved(peer, input, now));
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

    /// A clock set once and left alone is the bug this replaced: monotonic time does
    /// not count suspend, so the server's idea of "now" falls behind and valid tokens
    /// start reading as issued in the future.
    #[test]
    fn the_clock_can_be_moved_forward() {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let at = Instant::now();
        server.set_identity_keys(Jwks::parse(r#"{"keys":[]}"#).unwrap(), 1_000, at);
        assert_eq!(server.unix_now(at), Some(1_000));

        server.set_clock(9_999, at);
        assert_eq!(
            server.unix_now(at),
            Some(9_999),
            "a later reading must win over the old anchor"
        );
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

    fn a_server_with_a_player_at_spawn() -> (Server, SocketAddr, usize) {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();

        // No cipher, so nothing actually goes out — the bookkeeping this exercises is
        // the same either way, and the encrypted path has its own tests.
        let spawned = server.stream_world(peer, SERVER_CHUNK_RADIUS, Instant::now());
        let Event::Spawned { columns, .. } = spawned else {
            unreachable!("stream_world reports a spawn")
        };
        (server, peer, columns)
    }

    fn moves_to(server: &mut Server, peer: SocketAddr, x: f32, z: f32) -> Vec<Event> {
        let input = player::AuthInput {
            pitch: 0.0,
            yaw: 0.0,
            x,
            y: SURFACE_HEIGHT as f32,
            z,
        };
        server.on_player_moved(peer, input, Instant::now())
    }

    fn streamed(events: &[Event]) -> Option<usize> {
        events.iter().find_map(|event| match event {
            Event::ChunksStreamed { columns, .. } => Some(*columns),
            _ => None,
        })
    }

    /// Spawn covers the square around the origin, and nothing beyond it.
    #[test]
    fn spawning_streams_the_view_distance_around_the_origin() {
        let (server, _, columns) = a_server_with_a_player_at_spawn();
        let side = (SERVER_CHUNK_RADIUS * 2 + 1) as usize;
        assert_eq!(columns, side * side);
        assert_eq!(server.loaded_columns(), side * side);
    }

    /// The M0 criterion: a player who walks keeps being sent world. Before this, the
    /// columns sent at spawn were all a player ever got, and walking far enough took
    /// them past the edge of everything they had.
    #[test]
    fn crossing_into_a_new_column_streams_the_ground_ahead() {
        let (mut server, peer, _) = a_server_with_a_player_at_spawn();

        // Still inside the column they spawned in: nothing to send.
        let events = moves_to(&mut server, peer, 15.0, 0.0);
        assert_eq!(streamed(&events), None, "same column, no streaming");

        // One block further is the next column, which brings one new row into view.
        let events = moves_to(&mut server, peer, 16.0, 0.0);
        let row = (SERVER_CHUNK_RADIUS * 2 + 1) as usize;
        assert_eq!(streamed(&events), Some(row));
    }

    /// A position is a float and a column is not: block -1 belongs to column -1, and
    /// truncating towards zero would put it in column 0 and stream nothing.
    #[test]
    fn walking_west_of_the_origin_crosses_a_column_too() {
        let (mut server, peer, _) = a_server_with_a_player_at_spawn();
        let events = moves_to(&mut server, peer, -0.5, 0.0);
        let row = (SERVER_CHUNK_RADIUS * 2 + 1) as usize;
        assert_eq!(streamed(&events), Some(row));
    }

    /// Ground already sent is not sent again. A player pacing back and forth over a
    /// boundary would otherwise re-send a row of columns at every step.
    #[test]
    fn walking_back_over_ground_already_sent_costs_nothing() {
        let (mut server, peer, _) = a_server_with_a_player_at_spawn();
        moves_to(&mut server, peer, 16.0, 0.0);

        let events = moves_to(&mut server, peer, 8.0, 0.0);
        assert_eq!(streamed(&events), Some(0));
    }

    /// Far enough away, a column is forgotten — and coming back sends it again, because
    /// a client that unloaded it is waiting for it.
    #[test]
    fn columns_left_far_behind_are_sent_again_on_the_way_back() {
        let (mut server, peer, _) = a_server_with_a_player_at_spawn();

        let far = ((SERVER_CHUNK_RADIUS + FORGET_MARGIN + 5) * level_chunk::CHUNK_WIDTH) as f32;
        moves_to(&mut server, peer, far, 0.0);

        let events = moves_to(&mut server, peer, 0.0, 0.0);
        let side = (SERVER_CHUNK_RADIUS * 2 + 1) as usize;
        assert_eq!(
            streamed(&events),
            Some(side * side),
            "nothing around the origin was still remembered"
        );
    }

    /// The world is generated once and kept: walking the same ground twice must not
    /// generate it twice, which is what makes a change to it able to survive.
    #[test]
    fn walking_does_not_regenerate_ground_already_generated() {
        let (mut server, peer, _) = a_server_with_a_player_at_spawn();
        moves_to(&mut server, peer, 16.0, 0.0);
        let after_walking = server.loaded_columns();

        moves_to(&mut server, peer, 0.0, 0.0);
        assert_eq!(server.loaded_columns(), after_walking);
    }

    /// A player who leaves takes their bookkeeping with them. Keeping it would leak a
    /// set of columns per session for as long as the server runs.
    #[test]
    fn a_disconnect_forgets_the_player() {
        let (mut server, peer, _) = a_server_with_a_player_at_spawn();
        assert!(server.players.contains_key(&peer));

        server.players.remove(&peer);
        let events = moves_to(&mut server, peer, 16.0, 0.0);
        assert_eq!(streamed(&events), None, "a stranger streams nothing");
        assert!(matches!(events.as_slice(), [Event::PlayerMoved { .. }]));
    }

    #[test]
    fn a_payload_that_is_not_a_batch_is_reported() {
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();
        let events = server.on_payload(peer, &[0x00, 0x01], Instant::now());
        assert!(matches!(events.as_slice(), [Event::Undecodable(_, _)]));
    }
}
