//! The server side: answers the offline phase and owns one [`Session`] per peer.
//!
//! Sans-io like [`Session`] (ADR-012). Datagrams and the time go in; datagrams and
//! [`Event`]s come out.
//!
//! Offline packets are answered before any peer has proved anything about itself. Only
//! the ping is rate limited, because only the ping amplifies: a 33-byte ping draws a
//! pong of a hundred or more, while `OpenConnectionRequest1` is padded to the MTU and
//! draws 28 bytes back. Rate limiting the open sequence instead breaks it — request 1
//! and request 2 arrive back to back from the same address.
//!
//! Capping the advertisement bounds how much a pong can amplify; the interval bounds
//! how often a forged source can ask. See `SECURITY.md`.

use crate::address;
use crate::connect::{
    ID_INCOMPATIBLE_PROTOCOL_VERSION, ID_OPEN_CONNECTION_REPLY_1, ID_OPEN_CONNECTION_REPLY_2,
    ID_OPEN_CONNECTION_REQUEST_1, ID_OPEN_CONNECTION_REQUEST_2, PROTOCOL_VERSION, payload_limit,
};
use crate::offline::{ID_UNCONNECTED_PING, ID_UNCONNECTED_PONG};
use crate::session::{Closed, Config, Session, SessionError, State};
use crate::wire::{Reader, Writer};
use crate::{MAGIC, MAX_MTU, UDP_IP_OVERHEAD};
use std::collections::{HashMap, VecDeque};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Something that happened to a peer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The peer completed the handshake.
    Connected(SocketAddr),
    /// The peer sent a payload.
    Payload(SocketAddr, Vec<u8>),
    /// The peer went away. Carries why: a peer that left and a peer we gave up on
    /// point at opposite bugs.
    Disconnected(SocketAddr, Closed),
}

/// What the listener will spend before a peer has proved anything.
#[derive(Debug, Clone)]
pub struct ListenerConfig {
    /// Per-session settings.
    pub session: Config,
    /// Longest advertisement string we will send, bounding the amplification factor.
    pub max_advertisement: usize,
    /// Shortest gap between pong replies to one source address.
    pub ping_interval: Duration,
    /// Source addresses tracked for rate limiting.
    pub max_tracked_sources: usize,
    /// Sessions accepted at once.
    pub max_sessions: usize,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            session: Config::default(),
            max_advertisement: 256,
            ping_interval: Duration::from_millis(100),
            max_tracked_sources: 4096,
            max_sessions: 128,
        }
    }
}

/// A RakNet server.
#[derive(Debug)]
pub struct Listener {
    local: SocketAddr,
    guid: i64,
    advertisement: String,
    config: ListenerConfig,
    sessions: HashMap<SocketAddr, Session>,
    outbox: VecDeque<(SocketAddr, Arc<[u8]>)>,
    last_ping: HashMap<IpAddr, Instant>,
}

impl Listener {
    /// A listener advertising `advertisement`, truncated to the configured cap.
    pub fn new(local: SocketAddr, guid: i64, advertisement: &str, config: ListenerConfig) -> Self {
        let mut advertisement = advertisement.to_owned();
        advertisement.truncate(config.max_advertisement);
        Self {
            local,
            guid,
            advertisement,
            config,
            sessions: HashMap::new(),
            outbox: VecDeque::new(),
            last_ping: HashMap::new(),
        }
    }

    /// Connected and connecting peers.
    pub fn sessions(&self) -> usize {
        self.sessions.len()
    }

    /// The next datagram to put on the socket, and where it goes.
    pub fn poll_transmit(&mut self) -> Option<(SocketAddr, Arc<[u8]>)> {
        self.outbox.pop_front()
    }

    /// Queues a payload for a connected peer.
    pub fn send(
        &mut self,
        peer: SocketAddr,
        payload: Vec<u8>,
        now: Instant,
    ) -> Result<(), SessionError> {
        let session = self.sessions.get_mut(&peer).ok_or(SessionError::Closed)?;
        session.send(payload, now)?;
        Self::drain(&mut self.outbox, peer, session);
        Ok(())
    }

    fn drain(
        outbox: &mut VecDeque<(SocketAddr, Arc<[u8]>)>,
        peer: SocketAddr,
        session: &mut Session,
    ) {
        while let Some(datagram) = session.poll_transmit() {
            outbox.push_back((peer, datagram));
        }
    }

    fn reply(&mut self, to: SocketAddr, bytes: Vec<u8>) {
        self.outbox
            .push_back((to, Arc::from(bytes.into_boxed_slice())));
    }

    /// Whether this source may have a pong right now.
    fn allow_ping(&mut self, from: SocketAddr, now: Instant) -> bool {
        if self.last_ping.len() >= self.config.max_tracked_sources {
            let interval = self.config.ping_interval;
            self.last_ping
                .retain(|_, seen| now.duration_since(*seen) < interval);
        }
        match self.last_ping.get(&from.ip()) {
            Some(seen) if now.duration_since(*seen) < self.config.ping_interval => false,
            _ => {
                self.last_ping.insert(from.ip(), now);
                true
            }
        }
    }

    /// Feeds one datagram in.
    pub fn receive(&mut self, from: SocketAddr, bytes: &[u8], now: Instant) -> Vec<Event> {
        if let Some(session) = self.sessions.get_mut(&from) {
            let before = session.state();
            let payloads = session.receive(bytes, now).unwrap_or_default();
            // On the transition, not on every datagram after it: an application reading
            // Connected as "a player joined" would otherwise see one join per packet.
            let connected = before != State::Connected && session.state() == State::Connected;
            let closed = session.closed_because();
            Self::drain(&mut self.outbox, from, session);

            let mut events: Vec<Event> = payloads
                .into_iter()
                .map(|payload| Event::Payload(from, payload))
                .collect();
            if let Some(reason) = closed {
                self.sessions.remove(&from);
                events.push(Event::Disconnected(from, reason));
            } else if connected {
                events.insert(0, Event::Connected(from));
            }
            return events;
        }

        self.offline(from, bytes, now);
        Vec::new()
    }

    fn offline(&mut self, from: SocketAddr, bytes: &[u8], now: Instant) {
        let Some(&id) = bytes.first() else {
            return;
        };
        if !matches!(
            id,
            ID_UNCONNECTED_PING | ID_OPEN_CONNECTION_REQUEST_1 | ID_OPEN_CONNECTION_REQUEST_2
        ) {
            return;
        }
        match id {
            ID_UNCONNECTED_PING if !self.allow_ping(from, now) => {}
            ID_UNCONNECTED_PING => {
                let mut r = Reader::new(bytes);
                let Ok(time) = (|| {
                    r.u8()?;
                    r.i64()
                })() else {
                    return;
                };
                let mut w = Writer::new();
                w.u8(ID_UNCONNECTED_PONG)
                    .i64(time)
                    .i64(self.guid)
                    .bytes(&MAGIC)
                    .u16(u16::try_from(self.advertisement.len()).unwrap_or(u16::MAX))
                    .bytes(self.advertisement.as_bytes());
                self.reply(from, w.finish());
            }
            ID_OPEN_CONNECTION_REQUEST_1 => {
                let mut r = Reader::new(bytes);
                let Ok(version) = (|| {
                    r.u8()?;
                    r.array::<16>()?;
                    r.u8()
                })() else {
                    return;
                };

                if version != PROTOCOL_VERSION {
                    let mut w = Writer::new();
                    w.u8(ID_INCOMPATIBLE_PROTOCOL_VERSION)
                        .u8(PROTOCOL_VERSION)
                        .bytes(&MAGIC)
                        .i64(self.guid);
                    self.reply(from, w.finish());
                    return;
                }

                // The advertised MTU counts the IP and UDP headers, which is why this
                // adds them back to the size that actually arrived.
                let mtu = (bytes.len() + UDP_IP_OVERHEAD).min(MAX_MTU + UDP_IP_OVERHEAD);
                let mut w = Writer::new();
                w.u8(ID_OPEN_CONNECTION_REPLY_1)
                    .bytes(&MAGIC)
                    .i64(self.guid)
                    .u8(0)
                    .u16(u16::try_from(mtu).unwrap_or(u16::MAX));
                self.reply(from, w.finish());
            }
            ID_OPEN_CONNECTION_REQUEST_2 => {
                if self.sessions.len() >= self.config.max_sessions {
                    return;
                }
                let mut r = Reader::new(bytes);
                let Ok(mtu) = (|| {
                    r.u8()?;
                    r.array::<16>()?;
                    address::read(&mut r).map_err(|_| crate::wire::DecodeError {
                        needed: 0,
                        available: 0,
                    })?;
                    r.u16()
                })() else {
                    return;
                };

                let mut w = Writer::new();
                w.u8(ID_OPEN_CONNECTION_REPLY_2)
                    .bytes(&MAGIC)
                    .i64(self.guid);
                address::write(&mut w, from);
                w.u16(mtu).u8(0);
                self.reply(from, w.finish());

                let mut config = self.config.session;
                config.payload_limit = payload_limit(mtu).max(576);
                self.sessions
                    .insert(from, Session::new(from, self.local, config, now));
            }
            _ => {}
        }
    }

    /// Drives every session and drops the ones that ended.
    pub fn tick(&mut self, now: Instant) -> Vec<Event> {
        let mut events = Vec::new();
        let mut gone = Vec::new();

        for (&peer, session) in &mut self.sessions {
            session.tick(now);
            while let Some(datagram) = session.poll_transmit() {
                self.outbox.push_back((peer, datagram));
            }
            if let Some(reason) = session.closed_because() {
                gone.push((peer, reason));
            }
        }

        for (peer, reason) in gone {
            self.sessions.remove(&peer);
            events.push(Event::Disconnected(peer, reason));
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connect::{decode_reply_1, decode_reply_2, encode_request_1, encode_request_2};
    use crate::offline::{decode_unconnected_pong, encode_unconnected_ping};

    fn listener() -> Listener {
        Listener::new(
            "198.51.100.1:19132"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
            0x1234,
            "MCPE;test;1001;1.26.30;0;10;1234;;Survival",
            ListenerConfig::default(),
        )
    }

    fn peer(n: u16) -> SocketAddr {
        format!("203.0.113.{}:41234", n % 250 + 1)
            .parse()
            .unwrap_or_else(|_| unreachable!())
    }

    #[test]
    fn a_ping_is_answered_with_the_advertisement() {
        let mut l = listener();
        let t = Instant::now();
        l.receive(peer(1), &encode_unconnected_ping(42, 7), t);

        let (to, datagram) = l.poll_transmit().unwrap();
        assert_eq!(to, peer(1));
        let pong = decode_unconnected_pong(&datagram).unwrap();
        assert_eq!(pong.time, 42);
        assert_eq!(pong.server_guid, 0x1234);
        assert!(pong.advertisement.starts_with("MCPE;"));
    }

    /// A long advertisement is what turns a pong into a reflector payload.
    #[test]
    fn the_advertisement_is_capped() {
        let config = ListenerConfig {
            max_advertisement: 16,
            ..ListenerConfig::default()
        };
        let l = Listener::new(
            "198.51.100.1:19132"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
            1,
            &"x".repeat(10_000),
            config,
        );
        assert_eq!(l.advertisement.len(), 16);
    }

    #[test]
    fn the_open_sequence_is_not_rate_limited() {
        let mut l = listener();
        let t = Instant::now();

        l.receive(peer(1), &encode_request_1(1200).unwrap(), t);
        assert!(l.poll_transmit().is_some());

        let request = encode_request_2("198.51.100.1:19132".parse().unwrap(), 1228, 9, None);
        l.receive(peer(1), &request, t);
        assert!(
            l.poll_transmit().is_some(),
            "request 2 follows request 1 immediately"
        );
    }

    #[test]
    fn pings_are_rate_limited_per_source() {
        let mut l = listener();
        let t = Instant::now();

        l.receive(peer(1), &encode_unconnected_ping(1, 1), t);
        assert!(l.poll_transmit().is_some());

        l.receive(peer(1), &encode_unconnected_ping(2, 1), t);
        assert!(l.poll_transmit().is_none(), "same source, too soon");

        l.receive(peer(2), &encode_unconnected_ping(3, 1), t);
        assert!(l.poll_transmit().is_some(), "a different source is fine");

        l.receive(
            peer(1),
            &encode_unconnected_ping(4, 1),
            t + Duration::from_secs(1),
        );
        assert!(l.poll_transmit().is_some(), "the gap has passed");
    }

    #[test]
    fn a_wrong_protocol_version_is_told_which_one_to_use() {
        let mut l = listener();
        let t = Instant::now();
        let mut request = encode_request_1(600).unwrap();
        request[17] = PROTOCOL_VERSION + 1;

        l.receive(peer(1), &request, t);
        let (_, datagram) = l.poll_transmit().unwrap();
        assert_eq!(datagram[0], ID_INCOMPATIBLE_PROTOCOL_VERSION);
        assert_eq!(datagram[1], PROTOCOL_VERSION);
    }

    /// Mirrors what the probe measured against a real server: the reply is the
    /// datagram that arrived plus the IP and UDP headers.
    #[test]
    fn the_reply_mtu_adds_the_headers_back() {
        let mut l = listener();
        let t = Instant::now();
        l.receive(peer(1), &encode_request_1(1200).unwrap(), t);

        let (_, datagram) = l.poll_transmit().unwrap();
        let reply = decode_reply_1(&datagram).unwrap();
        assert_eq!(reply.mtu, 1228);
        assert_eq!(payload_limit(reply.mtu), 1200);
    }

    #[test]
    fn request_2_opens_a_session() {
        let mut l = listener();
        let t = Instant::now();

        l.receive(peer(1), &encode_request_1(1200).unwrap(), t);
        let _ = l.poll_transmit();
        assert_eq!(l.sessions(), 0);

        // Back to back, as a real client sends them. Rate limiting the open sequence
        // here is what broke the handshake against a live socket.
        let request = encode_request_2("198.51.100.1:19132".parse().unwrap(), 1228, 9, None);
        l.receive(peer(1), &request, t);

        let (_, datagram) = l.poll_transmit().unwrap();
        let reply = decode_reply_2(&datagram).unwrap();
        assert_eq!(reply.mtu, 1228);
        assert_eq!(reply.client_addr, peer(1));
        assert_eq!(l.sessions(), 1);
    }

    #[test]
    fn sessions_are_capped() {
        let config = ListenerConfig {
            max_sessions: 2,
            ping_interval: Duration::ZERO,
            ..ListenerConfig::default()
        };
        let mut l = Listener::new(
            "198.51.100.1:19132"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
            1,
            "MCPE;x",
            config,
        );
        let t = Instant::now();
        let request = encode_request_2("198.51.100.1:19132".parse().unwrap(), 1228, 9, None);

        for n in 1..=5 {
            l.receive(peer(n), &request, t);
        }
        assert_eq!(l.sessions(), 2);
    }

    /// Connected fires once, on the transition.
    #[test]
    fn connected_is_reported_once() {
        use crate::datagram::{Datagram, FrameSet};
        use crate::frame::{Frame, Reliability};
        use crate::online::ID_NEW_INCOMING_CONNECTION;

        let mut l = listener();
        let t = Instant::now();
        let request = encode_request_2("198.51.100.1:19132".parse().unwrap(), 1228, 9, None);
        l.receive(peer(1), &request, t);

        let complete = |sequence: u32, reliable: u32, order: u32| {
            let mut w = Writer::new();
            Datagram::FrameSet(FrameSet {
                sequence,
                frames: vec![Frame {
                    reliability: Reliability::ReliableOrdered,
                    reliable_index: reliable,
                    sequence_index: 0,
                    order_index: order,
                    order_channel: 0,
                    split: None,
                    payload: vec![ID_NEW_INCOMING_CONNECTION],
                }],
            })
            .encode(&mut w);
            w.finish()
        };

        let events = l.receive(peer(1), &complete(0, 0, 0), t);
        assert_eq!(events, vec![Event::Connected(peer(1))]);

        let events = l.receive(peer(1), &complete(1, 1, 1), t);
        assert!(events.is_empty(), "already connected: {events:?}");
    }

    #[test]
    fn a_silent_session_is_dropped_and_reported() {
        let mut l = listener();
        let t = Instant::now();
        let request = encode_request_2("198.51.100.1:19132".parse().unwrap(), 1228, 9, None);
        l.receive(peer(1), &request, t);
        assert_eq!(l.sessions(), 1);

        let events = l.tick(t + Duration::from_secs(30));
        assert_eq!(events, vec![Event::Disconnected(peer(1), Closed::Timeout)]);
        assert_eq!(l.sessions(), 0);
    }

    #[test]
    fn junk_is_ignored_without_a_reply() {
        let mut l = listener();
        let t = Instant::now();
        for bytes in [vec![], vec![0xff], vec![0x42; 40]] {
            l.receive(peer(1), &bytes, t);
        }
        assert!(l.poll_transmit().is_none());
    }
}
