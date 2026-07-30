//! Keeping reliable datagrams until the peer acknowledges them.
//!
//! The retransmission timeout follows RFC 6298: a smoothed round trip estimate plus
//! four times its variation, doubled on each retry. A fixed timeout is either slower
//! than the link or faster, and both are bad — too long stalls after every loss, too
//! short floods a slow path with duplicates that make the congestion worse.
//!
//! Samples are only taken from datagrams that were sent once. A retransmitted one
//! cannot say which copy the ACK answered, so measuring it corrupts the estimate
//! (Karn's algorithm).

use crate::datagram::Acknowledgement;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Round trip estimate, in RFC 6298 terms.
#[derive(Debug, Clone)]
pub struct Rtt {
    smoothed: Option<Duration>,
    variation: Duration,
    min: Duration,
    max: Duration,
}

impl Rtt {
    /// An estimator clamped to `[min, max]`, starting at `max` until it has a sample.
    pub fn new(min: Duration, max: Duration) -> Self {
        Self {
            smoothed: None,
            variation: Duration::ZERO,
            min,
            max,
        }
    }

    /// Folds in one measurement.
    pub fn sample(&mut self, rtt: Duration) {
        match self.smoothed {
            None => {
                self.smoothed = Some(rtt);
                self.variation = rtt / 2;
            }
            Some(smoothed) => {
                let delta = smoothed.abs_diff(rtt);
                self.variation = (self.variation * 3 + delta) / 4;
                self.smoothed = Some((smoothed * 7 + rtt) / 8);
            }
        }
    }

    /// Current timeout.
    pub fn timeout(&self) -> Duration {
        match self.smoothed {
            None => self.max,
            Some(smoothed) => (smoothed + self.variation * 4).clamp(self.min, self.max),
        }
    }

    /// The smoothed estimate, once there has been a sample.
    pub fn smoothed(&self) -> Option<Duration> {
        self.smoothed
    }
}

/// What a session will spend holding unacknowledged datagrams.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Floor for the timeout, so a fast link does not retransmit on jitter alone.
    pub min_rto: Duration,
    /// Ceiling for the timeout, and the starting value before any sample.
    pub max_rto: Duration,
    /// Retries before the peer is considered gone.
    pub max_retries: u32,
    /// Unacknowledged datagrams held at once. A peer that stops acknowledging must not
    /// be able to grow this without limit — see the queue policy in `ARCHITECTURE.md`.
    pub max_in_flight: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            min_rto: Duration::from_millis(100),
            max_rto: Duration::from_secs(2),
            max_retries: 8,
            max_in_flight: 512,
        }
    }
}

/// The peer is holding more unacknowledged datagrams than [`Limits::max_in_flight`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowFull;

impl std::fmt::Display for WindowFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "too many unacknowledged datagrams in flight")
    }
}

impl std::error::Error for WindowFull {}

#[derive(Debug)]
struct InFlight {
    datagram: Arc<[u8]>,
    sent_at: Instant,
    retries: u32,
}

/// Holds sent datagrams until they are acknowledged, and says when to send them again.
///
/// Every method takes the current time rather than reading a clock, so the whole thing
/// is testable without sleeping.
#[derive(Debug)]
pub struct Retransmitter {
    limits: Limits,
    rtt: Rtt,
    in_flight: BTreeMap<u32, InFlight>,
    dead: bool,
}

impl Retransmitter {
    /// A retransmitter with the given limits.
    pub fn new(limits: Limits) -> Self {
        Self {
            rtt: Rtt::new(limits.min_rto, limits.max_rto),
            limits,
            in_flight: BTreeMap::new(),
            dead: false,
        }
    }

    /// The round trip estimate.
    pub fn rtt(&self) -> &Rtt {
        &self.rtt
    }

    /// Datagrams awaiting acknowledgement.
    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    /// Whether a datagram has been retried past [`Limits::max_retries`].
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// Records a datagram as sent.
    pub fn track(
        &mut self,
        sequence: u32,
        datagram: Arc<[u8]>,
        now: Instant,
    ) -> Result<(), WindowFull> {
        if self.in_flight.len() >= self.limits.max_in_flight {
            return Err(WindowFull);
        }
        self.in_flight.insert(
            sequence,
            InFlight {
                datagram,
                sent_at: now,
                retries: 0,
            },
        );
        Ok(())
    }

    /// Clears acknowledged datagrams and updates the estimate.
    pub fn on_ack(&mut self, ack: &Acknowledgement, now: Instant) {
        for &(start, end) in &ack.ranges {
            // Walking the map rather than the range keeps a peer's sixteen-million-wide
            // record from costing sixteen million iterations.
            let acked: Vec<u32> = self
                .in_flight
                .range(start..=end)
                .map(|(&sequence, _)| sequence)
                .collect();

            for sequence in acked {
                let Some(entry) = self.in_flight.remove(&sequence) else {
                    continue;
                };
                if entry.retries == 0 {
                    self.rtt.sample(now.duration_since(entry.sent_at));
                }
            }
        }
    }

    /// Datagrams the peer reported missing, to send again straight away.
    pub fn on_nack(&mut self, nack: &Acknowledgement, now: Instant) -> Vec<Arc<[u8]>> {
        let mut out = Vec::new();
        for &(start, end) in &nack.ranges {
            let missing: Vec<u32> = self
                .in_flight
                .range(start..=end)
                .map(|(&sequence, _)| sequence)
                .collect();

            for sequence in missing {
                if let Some(entry) = self.in_flight.get_mut(&sequence) {
                    entry.retries += 1;
                    entry.sent_at = now;
                    out.push(Arc::clone(&entry.datagram));
                }
            }
        }
        self.check_retries();
        out
    }

    /// Datagrams whose timeout has passed, to send again.
    pub fn due(&mut self, now: Instant) -> Vec<Arc<[u8]>> {
        let base = self.rtt.timeout();
        let max = self.limits.max_rto;
        let mut out = Vec::new();

        for entry in self.in_flight.values_mut() {
            let backoff = base
                .saturating_mul(1u32.checked_shl(entry.retries).unwrap_or(u32::MAX))
                .min(max);
            if now.duration_since(entry.sent_at) >= backoff {
                entry.retries += 1;
                entry.sent_at = now;
                out.push(Arc::clone(&entry.datagram));
            }
        }

        self.check_retries();
        out
    }

    fn check_retries(&mut self) {
        if self
            .in_flight
            .values()
            .any(|entry| entry.retries > self.limits.max_retries)
        {
            self.dead = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn datagram(byte: u8) -> Arc<[u8]> {
        Arc::from(vec![byte; 32].into_boxed_slice())
    }

    fn limits() -> Limits {
        Limits {
            min_rto: Duration::from_millis(100),
            max_rto: Duration::from_secs(2),
            max_retries: 3,
            max_in_flight: 4,
        }
    }

    #[test]
    fn timeout_starts_at_the_ceiling() {
        let rtt = Rtt::new(Duration::from_millis(100), Duration::from_secs(2));
        assert_eq!(rtt.timeout(), Duration::from_secs(2));
        assert_eq!(rtt.smoothed(), None);
    }

    #[test]
    fn the_first_sample_seeds_the_estimate() {
        let mut rtt = Rtt::new(Duration::from_millis(10), Duration::from_secs(2));
        rtt.sample(Duration::from_millis(200));
        assert_eq!(rtt.smoothed(), Some(Duration::from_millis(200)));
        // 200 + 4 * 100
        assert_eq!(rtt.timeout(), Duration::from_millis(600));
    }

    #[test]
    fn the_estimate_converges_on_a_steady_link() {
        let mut rtt = Rtt::new(Duration::from_millis(10), Duration::from_secs(30));
        for _ in 0..40 {
            rtt.sample(Duration::from_millis(80));
        }
        let smoothed = rtt.smoothed().unwrap_or_default();
        assert!(
            smoothed.abs_diff(Duration::from_millis(80)) < Duration::from_millis(2),
            "{smoothed:?}"
        );
        assert!(rtt.timeout() < Duration::from_millis(120));
    }

    #[test]
    fn the_timeout_is_clamped_both_ways() {
        let min = Duration::from_millis(100);
        let max = Duration::from_millis(500);

        let mut fast = Rtt::new(min, max);
        for _ in 0..20 {
            fast.sample(Duration::from_micros(200));
        }
        assert_eq!(fast.timeout(), min);

        let mut slow = Rtt::new(min, max);
        for _ in 0..20 {
            slow.sample(Duration::from_secs(9));
        }
        assert_eq!(slow.timeout(), max);
    }

    #[test]
    fn an_ack_clears_the_datagram_and_measures_the_trip() {
        let mut r = Retransmitter::new(limits());
        let t = Instant::now();
        r.track(1, datagram(1), t).unwrap();

        r.on_ack(&Acknowledgement::single(1), t + Duration::from_millis(120));
        assert_eq!(r.in_flight(), 0);
        assert_eq!(r.rtt().smoothed(), Some(Duration::from_millis(120)));
    }

    /// Karn's algorithm: an ACK for a retransmitted datagram cannot say which copy it
    /// answered, so it must not become a sample.
    #[test]
    fn a_retransmitted_datagram_is_not_measured() {
        let mut r = Retransmitter::new(limits());
        let t = Instant::now();
        r.track(1, datagram(1), t).unwrap();

        assert_eq!(r.due(t + Duration::from_secs(3)).len(), 1);
        r.on_ack(
            &Acknowledgement::single(1),
            t + Duration::from_secs(3) + Duration::from_millis(50),
        );

        assert_eq!(r.in_flight(), 0);
        assert_eq!(r.rtt().smoothed(), None, "no sample from a retransmission");
    }

    #[test]
    fn nothing_is_due_before_the_timeout() {
        let mut r = Retransmitter::new(limits());
        let t = Instant::now();
        r.track(1, datagram(1), t).unwrap();
        assert!(r.due(t + Duration::from_millis(50)).is_empty());
        assert_eq!(r.in_flight(), 1);
    }

    #[test]
    fn a_nack_resends_immediately() {
        let mut r = Retransmitter::new(limits());
        let t = Instant::now();
        r.track(7, datagram(7), t).unwrap();

        let resent = r.on_nack(&Acknowledgement::single(7), t);
        assert_eq!(resent.len(), 1);
        assert_eq!(resent[0][0], 7);
        assert_eq!(
            r.in_flight(),
            1,
            "still unacknowledged until an ACK arrives"
        );
    }

    #[test]
    fn a_nack_for_something_unsent_is_ignored() {
        let mut r = Retransmitter::new(limits());
        let t = Instant::now();
        r.track(1, datagram(1), t).unwrap();
        assert!(r.on_nack(&Acknowledgement::single(99), t).is_empty());
    }

    /// Each retry waits twice as long, up to the ceiling.
    #[test]
    fn retries_back_off() {
        let mut r = Retransmitter::new(limits());
        let mut t = Instant::now();
        r.track(1, datagram(1), t).unwrap();

        // No sample yet, so the base timeout is max_rto: 2s, then 4s clamped to 2s.
        assert!(r.due(t + Duration::from_millis(1999)).is_empty());
        t += Duration::from_secs(2);
        assert_eq!(r.due(t).len(), 1);
        assert!(r.due(t + Duration::from_millis(1999)).is_empty());
    }

    #[test]
    fn a_peer_that_never_acknowledges_is_declared_gone() {
        let mut r = Retransmitter::new(limits());
        let mut t = Instant::now();
        r.track(1, datagram(1), t).unwrap();

        for _ in 0..limits().max_retries + 1 {
            t += Duration::from_secs(3);
            r.due(t);
        }
        assert!(r.is_dead());
    }

    #[test]
    fn the_window_stops_an_unbounded_backlog() {
        let mut r = Retransmitter::new(limits());
        let t = Instant::now();
        for sequence in 0..4 {
            r.track(sequence, datagram(0), t).unwrap();
        }
        assert_eq!(r.track(4, datagram(0), t), Err(WindowFull));
    }

    /// A record can span sixteen million sequence numbers; clearing it must cost what
    /// we actually hold, not what the peer claimed.
    #[test]
    fn a_huge_ack_range_clears_only_what_is_held() {
        let mut r = Retransmitter::new(limits());
        let t = Instant::now();
        for sequence in 0..4 {
            r.track(sequence, datagram(0), t).unwrap();
        }
        r.on_ack(
            &Acknowledgement {
                ranges: vec![(0, 0xff_ffff)],
            },
            t + Duration::from_millis(10),
        );
        assert_eq!(r.in_flight(), 0);
    }
}
