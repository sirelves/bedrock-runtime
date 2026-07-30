//! Two sessions talking over a link that loses, duplicates and reorders.
//!
//! This is what the sans-io shape buys (ADR-012): a megabyte crosses a 12% loss link
//! with reordering and duplication, and the whole thing runs in milliseconds of real
//! time because the clock is a variable.
//!
//! The generator is a fixed-seed xorshift, so a failure here reproduces exactly.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bedrock_raknet::session::{Config, Session};
use std::net::SocketAddr;
use std::time::{Duration, Instant};

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// True with probability `percent`.
    fn chance(&mut self, percent: u32) -> bool {
        self.next() % 100 < percent
    }
}

struct Link {
    rng: Rng,
    loss: u32,
    duplicate: u32,
    reorder: u32,
    queue: Vec<Vec<u8>>,
    dropped: usize,
}

impl Link {
    fn new(seed: u32, loss: u32, duplicate: u32, reorder: u32) -> Self {
        Self {
            rng: Rng(seed),
            loss,
            duplicate,
            reorder,
            queue: Vec::new(),
            dropped: 0,
        }
    }

    fn offer(&mut self, datagram: Vec<u8>) {
        if self.rng.chance(self.loss) {
            self.dropped += 1;
            return;
        }
        if self.rng.chance(self.duplicate) {
            self.queue.push(datagram.clone());
        }
        if self.rng.chance(self.reorder) && !self.queue.is_empty() {
            let at = self.rng.next() as usize % self.queue.len();
            self.queue.insert(at, datagram);
        } else {
            self.queue.push(datagram);
        }
    }

    fn drain(&mut self) -> Vec<Vec<u8>> {
        std::mem::take(&mut self.queue)
    }
}

fn config() -> Config {
    Config {
        // A megabyte is roughly 725 datagrams, so the window has to hold them.
        retransmit: bedrock_raknet::retransmit::Limits {
            max_in_flight: 2048,
            ..Default::default()
        },
        max_outbox: 2048,
        ..Config::default()
    }
}

fn endpoints(now: Instant) -> (Session, Session) {
    let a: SocketAddr = "203.0.113.5:41234".parse().unwrap();
    let b: SocketAddr = "198.51.100.1:19132".parse().unwrap();
    (
        Session::new(b, a, config(), now),
        Session::new(a, b, config(), now),
    )
}

/// Runs the link until `sender`'s payload has arrived or the step budget runs out.
fn exchange(payload: Vec<u8>, loss: u32, duplicate: u32, reorder: u32) -> (Vec<Vec<u8>>, usize) {
    let start = Instant::now();
    let (mut sender, mut receiver) = endpoints(start);
    let mut to_receiver = Link::new(0x1234_5678, loss, duplicate, reorder);
    let mut to_sender = Link::new(0x9e37_79b9, loss, duplicate, reorder);

    sender.send(payload, start).unwrap();

    let mut delivered = Vec::new();
    let mut now = start;

    for _ in 0..4000 {
        now += Duration::from_millis(5);

        while let Some(datagram) = sender.poll_transmit() {
            to_receiver.offer(datagram.to_vec());
        }
        for datagram in to_receiver.drain() {
            if let Ok(payloads) = receiver.receive(&datagram, now) {
                delivered.extend(payloads);
            }
        }

        while let Some(datagram) = receiver.poll_transmit() {
            to_sender.offer(datagram.to_vec());
        }
        for datagram in to_sender.drain() {
            let _ = sender.receive(&datagram, now);
        }

        sender.tick(now);
        receiver.tick(now);

        if !delivered.is_empty() && sender.in_flight() == 0 {
            break;
        }
    }

    (delivered, to_receiver.dropped + to_sender.dropped)
}

fn body(len: usize) -> Vec<u8> {
    let mut payload = vec![0xfeu8];
    payload.extend((0..len).map(|i| (i % 251) as u8));
    payload
}

#[test]
fn a_megabyte_crosses_a_lossy_link_intact() {
    let payload = body(1024 * 1024);
    let (delivered, dropped) = exchange(payload.clone(), 12, 3, 25);

    assert!(dropped > 0, "the link should actually have lost something");
    assert_eq!(delivered.len(), 1, "exactly one payload");
    assert_eq!(delivered[0], payload, "and it arrived byte for byte");
}

#[test]
fn a_perfect_link_delivers_immediately() {
    let payload = body(64 * 1024);
    let (delivered, dropped) = exchange(payload.clone(), 0, 0, 0);
    assert_eq!(dropped, 0);
    assert_eq!(delivered, vec![payload]);
}

#[test]
fn heavy_reordering_alone_does_not_reorder_payloads() {
    let payload = body(256 * 1024);
    let (delivered, _) = exchange(payload.clone(), 0, 0, 90);
    assert_eq!(delivered, vec![payload]);
}

#[test]
fn duplication_alone_delivers_once() {
    let payload = body(128 * 1024);
    let (delivered, _) = exchange(payload.clone(), 0, 50, 0);
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0], payload);
}

/// A payload needing more datagrams than the window is refused whole, leaving nothing
/// queued — a half-sent payload would have the peer reassembling a rest that never comes.
#[test]
fn an_oversized_payload_is_refused_atomically() {
    let now = Instant::now();
    let (mut sender, _) = endpoints(now);

    let too_big = body(2048 * 1500);
    assert!(sender.send(too_big, now).is_err());
    assert!(
        sender.poll_transmit().is_none(),
        "nothing may be queued when the payload was refused"
    );
    assert_eq!(sender.in_flight(), 0);

    assert!(sender.send(body(1000), now).is_ok(), "still usable after");
}
