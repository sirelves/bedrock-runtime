//! The per-peer state machine.
//!
//! Sans-io: it never touches a socket and never reads a clock. Datagrams and the
//! current time go in, datagrams and delivered payloads come out. See ADR-012 in
//! `docs/DECISIONS.md` for why.
//!
//! ```text
//! receive(bytes, now) ─► acks, reassembly, ordering ─► payloads for the caller
//! send(payload, now)  ─► split, frame, pack          ─┐
//! tick(now)           ─► retransmit, ack, keepalive  ─┴─► poll_transmit()
//! ```
//!
//! It answers RakNet's own packets itself — connection handshake, ping, disconnect —
//! so what reaches the caller is only the payloads a peer actually sent. The split is
//! by id: RakNet reserves everything below [`USER_PACKET_START`], and only ids at or
//! above it are handed up. Bedrock's game batches start with `0xFE`, comfortably above.
//!
//! That boundary is not cosmetic. Without it a payload whose first byte happens to be
//! `0x00` is indistinguishable from `ConnectedPing`, and a session will quietly answer
//! it with a pong instead of delivering it.
//!
//! Incoming NACKs are honoured; outgoing ones are not generated. A lost datagram is
//! recovered by the retransmission timeout either way, so generating NACKs is a
//! latency optimisation, and `PERFORMANCE.md` says those wait for a measurement.

use crate::address;
use crate::datagram::{Acknowledgement, DATAGRAM_HEADER_LEN, Datagram, DatagramError, FrameSet};
use crate::frame::Frame;
use crate::online::{
    ID_CONNECTED_PING, ID_CONNECTED_PONG, ID_CONNECTION_REQUEST, ID_DISCONNECT,
    ID_NEW_INCOMING_CONNECTION, decode_connected_pong, encode_connected_ping,
};
use crate::order::{Dedup, Ordering, OutOfWindow};
use crate::retransmit::{self, Retransmitter, WindowFull};
use crate::split::{self, Reassembler, SplitError, Splitter, fragment_capacity};
use crate::wire::{Reader, Writer};
use std::collections::VecDeque;
use std::fmt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// First message id belonging to the application rather than to RakNet.
///
/// RakNet reserves the ids below this for itself. A payload starting lower is a
/// control packet, and one it does not recognise is dropped rather than delivered —
/// guessing which of the two a low id meant is how a ping becomes a lost payload.
pub const USER_PACKET_START: u8 = 0x86;

/// How far a session has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Open, but the peer has not completed the handshake.
    Connecting,
    /// Handshake done; payloads flow.
    Connected,
    /// Finished, by disconnect or by giving up on the peer.
    Closed,
}

/// What a session will spend on one peer.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// Largest datagram we may build, from [`crate::connect::payload_limit`].
    pub payload_limit: usize,
    /// Reassembly bounds.
    pub split: split::Limits,
    /// Retransmission bounds.
    pub retransmit: retransmit::Limits,
    /// Ordered payloads held ahead of a gap.
    pub order_window: u32,
    /// Reliable indices remembered for deduplication.
    pub dedup_window: u32,
    /// Silence before we ping to check the peer is alive.
    pub keepalive: Duration,
    /// Silence before we give up on the peer.
    pub timeout: Duration,
    /// Datagrams queued for the socket. Bounds a peer that reads slower than we write.
    pub max_outbox: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            payload_limit: 1472,
            split: split::Limits::default(),
            retransmit: retransmit::Limits::default(),
            order_window: 2048,
            dedup_window: 4096,
            keepalive: Duration::from_secs(5),
            timeout: Duration::from_secs(20),
            max_outbox: 256,
        }
    }
}

/// Why a session refused something.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionError {
    /// The datagram did not decode.
    Datagram(DatagramError),
    /// A fragment was refused.
    Split(SplitError),
    /// An ordered payload was outside the window.
    Order(OutOfWindow),
    /// Too many unacknowledged datagrams.
    Window(WindowFull),
    /// The outbox is full.
    OutboxFull,
    /// The session is closed.
    Closed,
}

impl From<DatagramError> for SessionError {
    fn from(e: DatagramError) -> Self {
        Self::Datagram(e)
    }
}
impl From<SplitError> for SessionError {
    fn from(e: SplitError) -> Self {
        Self::Split(e)
    }
}
impl From<OutOfWindow> for SessionError {
    fn from(e: OutOfWindow) -> Self {
        Self::Order(e)
    }
}
impl From<WindowFull> for SessionError {
    fn from(e: WindowFull) -> Self {
        Self::Window(e)
    }
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Datagram(e) => write!(f, "{e}"),
            Self::Split(e) => write!(f, "{e}"),
            Self::Order(e) => write!(f, "{e}"),
            Self::Window(e) => write!(f, "{e}"),
            Self::OutboxFull => write!(f, "outbox is full"),
            Self::Closed => write!(f, "session is closed"),
        }
    }
}

impl std::error::Error for SessionError {}

/// One peer's connection.
#[derive(Debug)]
pub struct Session {
    config: Config,
    peer: SocketAddr,
    local: SocketAddr,
    state: State,

    next_sequence: u32,
    next_reliable: u32,
    next_order: u32,
    splitter: Splitter,
    retransmitter: Retransmitter,
    outbox: VecDeque<Arc<[u8]>>,

    reassembler: Reassembler,
    ordering: Ordering,
    dedup: Dedup,
    pending_acks: Vec<u32>,

    started: Instant,
    last_heard: Instant,
    last_ping: Instant,
}

impl Session {
    /// A session for a peer that has just finished opening a connection.
    pub fn new(peer: SocketAddr, local: SocketAddr, config: Config, now: Instant) -> Self {
        Self {
            retransmitter: Retransmitter::new(config.retransmit),
            reassembler: Reassembler::new(config.split),
            ordering: Ordering::new(config.order_window),
            dedup: Dedup::new(config.dedup_window),
            config,
            peer,
            local,
            state: State::Connecting,
            next_sequence: 0,
            next_reliable: 0,
            next_order: 0,
            splitter: Splitter::default(),
            outbox: VecDeque::new(),
            pending_acks: Vec::new(),
            started: now,
            last_heard: now,
            last_ping: now,
        }
    }

    /// How far the handshake has got.
    pub fn state(&self) -> State {
        self.state
    }

    /// The peer's address.
    pub fn peer(&self) -> SocketAddr {
        self.peer
    }

    /// Smoothed round trip time, once an acknowledgement has measured one.
    pub fn rtt(&self) -> Option<Duration> {
        self.retransmitter.rtt().smoothed()
    }

    /// Datagrams sent but not yet acknowledged.
    pub fn in_flight(&self) -> usize {
        self.retransmitter.in_flight()
    }

    /// Whether the session is finished and should be dropped.
    pub fn is_closed(&self) -> bool {
        self.state == State::Closed
    }

    /// Milliseconds since the session opened, as RakNet timestamps them.
    fn now_millis(&self, now: Instant) -> i64 {
        i64::try_from(now.duration_since(self.started).as_millis()).unwrap_or(i64::MAX)
    }

    /// The next datagram for the socket.
    pub fn poll_transmit(&mut self) -> Option<Arc<[u8]>> {
        self.outbox.pop_front()
    }

    /// Queues a payload for the peer, splitting it if it does not fit.
    pub fn send(&mut self, payload: Vec<u8>, now: Instant) -> Result<(), SessionError> {
        if self.state == State::Closed {
            return Err(SessionError::Closed);
        }
        let capacity = fragment_capacity(self.config.payload_limit);
        let frames = self.splitter.split(payload, capacity);
        self.send_frames(frames, now)
    }

    fn send_frames(&mut self, frames: Vec<Frame>, now: Instant) -> Result<(), SessionError> {
        let budget = self
            .config
            .payload_limit
            .saturating_sub(DATAGRAM_HEADER_LEN);

        let mut batch: Vec<Frame> = Vec::new();
        let mut used = 0usize;

        for mut frame in frames {
            frame.reliable_index = self.next_reliable;
            self.next_reliable = self.next_reliable.wrapping_add(1);
            frame.order_index = self.next_order;

            let size = frame.encoded_len();
            if !batch.is_empty() && used + size > budget {
                self.flush_batch(std::mem::take(&mut batch), now)?;
                used = 0;
            }
            used += size;
            batch.push(frame);
        }

        // Every fragment of one payload shares an order index; the reassembled payload
        // is what the peer orders, not the pieces.
        self.next_order = self.next_order.wrapping_add(1);

        if !batch.is_empty() {
            self.flush_batch(batch, now)?;
        }
        Ok(())
    }

    fn flush_batch(&mut self, frames: Vec<Frame>, now: Instant) -> Result<(), SessionError> {
        if self.outbox.len() >= self.config.max_outbox {
            return Err(SessionError::OutboxFull);
        }

        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);

        let mut w = Writer::new();
        Datagram::FrameSet(FrameSet { sequence, frames }).encode(&mut w);
        let datagram: Arc<[u8]> = Arc::from(w.finish().into_boxed_slice());

        self.retransmitter
            .track(sequence, Arc::clone(&datagram), now)?;
        self.outbox.push_back(datagram);
        Ok(())
    }

    fn queue_raw(&mut self, bytes: Vec<u8>) {
        if self.outbox.len() < self.config.max_outbox {
            self.outbox.push_back(Arc::from(bytes.into_boxed_slice()));
        }
    }

    /// Feeds one datagram in and returns the payloads it completed.
    pub fn receive(&mut self, bytes: &[u8], now: Instant) -> Result<Vec<Vec<u8>>, SessionError> {
        if self.state == State::Closed {
            return Err(SessionError::Closed);
        }
        self.last_heard = now;

        let set = match Datagram::decode(bytes)? {
            Datagram::Ack(ack) => {
                self.retransmitter.on_ack(&ack, now);
                return Ok(Vec::new());
            }
            Datagram::Nack(nack) => {
                for datagram in self.retransmitter.on_nack(&nack, now) {
                    if self.outbox.len() < self.config.max_outbox {
                        self.outbox.push_back(datagram);
                    }
                }
                return Ok(Vec::new());
            }
            Datagram::FrameSet(set) => set,
        };

        // Acknowledge even a duplicate: the peer is retransmitting because it never
        // heard the first acknowledgement.
        self.pending_acks.push(set.sequence);

        let mut delivered = Vec::new();
        for frame in set.frames {
            if frame.reliability.is_reliable() && !self.dedup.accept(frame.reliable_index) {
                continue;
            }
            let ordered = frame.reliability.is_ordered();
            let order_index = frame.order_index;

            let Some(payload) = self.reassembler.push(frame, now)? else {
                continue;
            };

            let payloads = if ordered {
                self.ordering.push(order_index, payload)?
            } else {
                vec![payload]
            };

            for payload in payloads {
                if let Some(payload) = self.handle_internal(payload, now)? {
                    delivered.push(payload);
                }
            }
        }
        Ok(delivered)
    }

    /// Answers RakNet's own packets. Returns anything meant for the caller.
    fn handle_internal(
        &mut self,
        payload: Vec<u8>,
        now: Instant,
    ) -> Result<Option<Vec<u8>>, SessionError> {
        match payload.first() {
            Some(&ID_CONNECTION_REQUEST) => {
                let mut r = Reader::new(&payload);
                // id, client guid, then the timestamp we echo back.
                let time = (|| {
                    r.u8().ok()?;
                    r.i64().ok()?;
                    r.i64().ok()
                })()
                .unwrap_or(0);
                let reply = self.encode_request_accepted(time, now);
                self.send(reply, now)?;
                Ok(None)
            }
            Some(&ID_NEW_INCOMING_CONNECTION) => {
                self.state = State::Connected;
                Ok(None)
            }
            Some(&ID_CONNECTED_PING) => {
                let mut r = Reader::new(&payload);
                let time = (|| {
                    r.u8().ok()?;
                    r.i64().ok()
                })()
                .unwrap_or(0);
                let mut w = Writer::new();
                w.u8(ID_CONNECTED_PONG).i64(time).i64(self.now_millis(now));
                let pong = w.finish();
                self.send(pong, now)?;
                Ok(None)
            }
            Some(&ID_CONNECTED_PONG) => {
                let _ = decode_connected_pong(&payload);
                Ok(None)
            }
            Some(&ID_DISCONNECT) => {
                self.state = State::Closed;
                Ok(None)
            }
            Some(&id) if id >= USER_PACKET_START => Ok(Some(payload)),
            _ => Ok(None),
        }
    }

    fn encode_request_accepted(&self, request_time: i64, now: Instant) -> Vec<u8> {
        let mut w = Writer::new();
        w.u8(crate::online::ID_CONNECTION_REQUEST_ACCEPTED);
        address::write(&mut w, self.peer);
        w.u16(0);
        for _ in 0..crate::online::SYSTEM_ADDRESS_COUNT {
            address::write(&mut w, self.local);
        }
        w.i64(request_time).i64(self.now_millis(now));
        w.finish()
    }

    /// Retransmits, acknowledges and checks the peer is still there.
    pub fn tick(&mut self, now: Instant) {
        if self.state == State::Closed {
            return;
        }

        if now.duration_since(self.last_heard) >= self.config.timeout
            || self.retransmitter.is_dead()
        {
            self.state = State::Closed;
            return;
        }

        self.flush_acks();

        for datagram in self.retransmitter.due(now) {
            if self.outbox.len() < self.config.max_outbox {
                self.outbox.push_back(datagram);
            }
        }

        self.reassembler.expire(now);

        if now.duration_since(self.last_ping) >= self.config.keepalive {
            self.last_ping = now;
            let ping = encode_connected_ping(self.now_millis(now));
            let _ = self.send(ping, now);
        }
    }

    fn flush_acks(&mut self) {
        if self.pending_acks.is_empty() {
            return;
        }
        let mut sequences = std::mem::take(&mut self.pending_acks);
        sequences.sort_unstable();
        sequences.dedup();

        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for sequence in sequences {
            match ranges.last_mut() {
                Some(last) if sequence == last.1 + 1 => last.1 = sequence,
                _ => ranges.push((sequence, sequence)),
            }
        }

        let mut w = Writer::new();
        Datagram::Ack(Acknowledgement { ranges }).encode(&mut w);
        self.queue_raw(w.finish());
    }

    /// Queues a disconnect and closes.
    pub fn close(&mut self, now: Instant) {
        if self.state == State::Closed {
            return;
        }
        let _ = self.send(vec![ID_DISCONNECT], now);
        self.state = State::Closed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Reliability;
    use crate::online::{ID_CONNECTION_REQUEST_ACCEPTED, encode_connection_request};

    fn addrs() -> (SocketAddr, SocketAddr) {
        (
            "203.0.113.5:41234"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
            "198.51.100.1:19132"
                .parse()
                .unwrap_or_else(|_| unreachable!()),
        )
    }

    fn session(now: Instant) -> Session {
        let (peer, local) = addrs();
        Session::new(peer, local, Config::default(), now)
    }

    /// Moves everything one side queued into the other, returning what was delivered.
    fn pump(from: &mut Session, to: &mut Session, now: Instant) -> Vec<Vec<u8>> {
        let mut delivered = Vec::new();
        while let Some(datagram) = from.poll_transmit() {
            if let Ok(payloads) = to.receive(&datagram, now) {
                delivered.extend(payloads);
            }
        }
        delivered
    }

    #[test]
    fn a_payload_survives_the_round_trip() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));
        let payload = b"\xfehello".to_vec();
        a.send(payload.clone(), t).unwrap();
        assert_eq!(pump(&mut a, &mut b, t), vec![payload]);
    }

    /// The boundary that keeps a payload from being mistaken for a control packet.
    /// A payload starting with 0x00 is ConnectedPing to RakNet, not application data.
    #[test]
    fn payloads_below_the_user_threshold_are_not_delivered() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));

        a.send(vec![0x00, 1, 2], t).unwrap();
        assert!(pump(&mut a, &mut b, t).is_empty());

        a.send(vec![USER_PACKET_START, 1, 2], t).unwrap();
        assert_eq!(pump(&mut a, &mut b, t), vec![vec![USER_PACKET_START, 1, 2]]);
    }

    #[test]
    fn a_large_payload_is_split_and_rebuilt() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));
        // Starts with 0xFE the way a Bedrock batch does; a payload starting below
        // USER_PACKET_START is RakNet's, not the caller's.
        let mut payload = vec![0xfeu8];
        payload.extend((0..200_000).map(|i| (i % 251) as u8));

        a.send(payload.clone(), t).unwrap();
        let delivered = pump(&mut a, &mut b, t);
        assert_eq!(delivered, vec![payload]);
    }

    #[test]
    fn payloads_arrive_in_order_when_datagrams_do_not() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));
        for n in 0..4u8 {
            a.send(vec![0xfe, n], t).unwrap();
        }

        let mut datagrams: Vec<Arc<[u8]>> = std::iter::from_fn(|| a.poll_transmit()).collect();
        datagrams.reverse();

        let mut delivered = Vec::new();
        for datagram in datagrams {
            delivered.extend(b.receive(&datagram, t).unwrap_or_default());
        }
        assert_eq!(
            delivered,
            vec![vec![0xfe, 0], vec![0xfe, 1], vec![0xfe, 2], vec![0xfe, 3]]
        );
    }

    #[test]
    fn a_duplicated_datagram_is_delivered_once() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));
        a.send(vec![0xfe, 1], t).unwrap();
        let datagram = a.poll_transmit().unwrap();

        assert_eq!(b.receive(&datagram, t).unwrap().len(), 1);
        assert!(b.receive(&datagram, t).unwrap().is_empty());
    }

    #[test]
    fn a_connection_request_is_answered_and_completes_the_handshake() {
        let t = Instant::now();
        let (mut client, mut server) = (session(t), session(t));

        client.send(encode_connection_request(7, 99), t).unwrap();
        assert!(pump(&mut client, &mut server, t).is_empty());
        assert_eq!(server.state(), State::Connecting);

        let reply = server.poll_transmit().unwrap();
        let decoded = Datagram::decode(&reply).unwrap();
        let Datagram::FrameSet(set) = decoded else {
            unreachable!("the reply is a frame set")
        };
        assert_eq!(set.frames[0].payload[0], ID_CONNECTION_REQUEST_ACCEPTED);

        server
            .receive(
                &{
                    let mut w = Writer::new();
                    Datagram::FrameSet(FrameSet {
                        sequence: 50,
                        frames: vec![Frame {
                            reliability: Reliability::ReliableOrdered,
                            reliable_index: 900,
                            sequence_index: 0,
                            order_index: 1,
                            order_channel: 0,
                            split: None,
                            payload: vec![ID_NEW_INCOMING_CONNECTION],
                        }],
                    })
                    .encode(&mut w);
                    w.finish()
                },
                t,
            )
            .unwrap();
        assert_eq!(server.state(), State::Connected);
    }

    #[test]
    fn a_ping_is_answered_with_a_pong() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));
        a.send(encode_connected_ping(1234), t).unwrap();
        assert!(pump(&mut a, &mut b, t).is_empty(), "ping is not a payload");

        let reply = b.poll_transmit().unwrap();
        let Datagram::FrameSet(set) = Datagram::decode(&reply).unwrap() else {
            unreachable!("the reply is a frame set")
        };
        let pong = decode_connected_pong(&set.frames[0].payload).unwrap();
        assert_eq!(pong.ping_time, 1234);
    }

    #[test]
    fn receiving_produces_an_acknowledgement_that_clears_the_sender() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));
        a.send(vec![0xfe, 1], t).unwrap();
        pump(&mut a, &mut b, t);

        b.tick(t);
        let ack = b.poll_transmit().unwrap();
        assert!(matches!(Datagram::decode(&ack), Ok(Datagram::Ack(_))));

        a.receive(&ack, t + Duration::from_millis(30)).unwrap();
        assert_eq!(
            a.rtt(),
            Some(Duration::from_millis(30)),
            "the ack should have measured the trip"
        );
    }

    #[test]
    fn an_unacknowledged_datagram_is_sent_again() {
        let t = Instant::now();
        let mut a = session(t);
        a.send(vec![0xfe, 1], t).unwrap();
        let _ = a.poll_transmit();

        a.tick(t + Duration::from_millis(50));
        assert!(a.poll_transmit().is_none(), "too early");

        a.tick(t + Duration::from_secs(3));
        assert!(a.poll_transmit().is_some(), "should have retransmitted");
    }

    #[test]
    fn a_silent_peer_closes_the_session() {
        let t = Instant::now();
        let mut a = session(t);
        a.tick(t + Duration::from_secs(19));
        assert_eq!(a.state(), State::Connecting);

        a.tick(t + Duration::from_secs(21));
        assert_eq!(a.state(), State::Closed);
    }

    #[test]
    fn silence_triggers_a_keepalive() {
        let t = Instant::now();
        let mut a = session(t);
        a.tick(t + Duration::from_secs(6));

        let datagram = a.poll_transmit().unwrap();
        let Datagram::FrameSet(set) = Datagram::decode(&datagram).unwrap() else {
            unreachable!("keepalive is a frame set")
        };
        assert_eq!(set.frames[0].payload[0], ID_CONNECTED_PING);
    }

    #[test]
    fn a_disconnect_closes_the_session() {
        let t = Instant::now();
        let (mut a, mut b) = (session(t), session(t));
        a.close(t);
        pump(&mut a, &mut b, t);
        assert_eq!(b.state(), State::Closed);
        assert_eq!(a.state(), State::Closed);
    }

    #[test]
    fn a_closed_session_refuses_work() {
        let t = Instant::now();
        let mut a = session(t);
        a.close(t);
        assert_eq!(a.send(vec![1], t), Err(SessionError::Closed));
        assert_eq!(a.receive(&[0x80, 0, 0, 0], t), Err(SessionError::Closed));
    }
}
