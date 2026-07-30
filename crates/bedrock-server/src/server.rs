//! Where the transport meets the game protocol.
//!
//! `bedrock-raknet` delivers payloads and `bedrock-protocol` says what they mean; this
//! is the only crate allowed to know both. Sans-io like the layers under it: datagrams
//! and the time in, datagrams and [`Event`]s out.
//!
//! It answers `RequestNetworkSettings` and nothing else yet. Everything past that is
//! reported so a capture can be taken of bytes no third-party documentation covers.

use bedrock_protocol::batch;
use bedrock_protocol::handshake::{
    ID_REQUEST_NETWORK_SETTINGS, NetworkSettings, RequestNetworkSettings,
};
use bedrock_protocol::version::{MINECRAFT_VERSION, PROTOCOL_VERSION};
use bedrock_raknet::listener::{Event as RakEvent, Listener, ListenerConfig};
use std::collections::HashSet;
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

    #[test]
    fn an_unknown_packet_is_reported_rather_than_dropped() {
        let payload = batch::encode(&[Packet::new(1, vec![1, 2, 3])]);
        let mut server = Server::new("0.0.0.0:19132".parse().unwrap(), 1, "MCPE;x");
        let peer: SocketAddr = "203.0.113.5:1234".parse().unwrap();

        let events = server.on_payload(peer, &payload, Instant::now());
        assert_eq!(
            events,
            vec![Event::Unhandled {
                peer,
                id: 1,
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
