//! Delivering reliable-ordered payloads in order, exactly once.
//!
//! Both structures here bound what a peer can make us hold. A peer that sends order
//! index 5000 and never sends 0 has to be stopped from buying unbounded buffering with
//! one frame, and a peer that replays old frames has to be stopped from having them
//! delivered twice.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// A peer sent something too far outside the window to be worth holding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfWindow {
    /// The index the peer sent.
    pub index: u32,
    /// The index we were waiting for.
    pub expected: u32,
}

impl fmt::Display for OutOfWindow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "order index {} is outside the window at {}",
            self.index, self.expected
        )
    }
}

impl std::error::Error for OutOfWindow {}

/// Buffers ordered payloads until the gaps ahead of them are filled.
#[derive(Debug)]
pub struct Ordering {
    expected: u32,
    buffered: BTreeMap<u32, Vec<u8>>,
    window: u32,
}

impl Ordering {
    /// Holds at most `window` payloads ahead of the one it is waiting for.
    ///
    /// The window doubles as the wrap guard: order indices are 24-bit and a session
    /// that reached the wrap would need sixteen million ordered frames, so anything
    /// that far ahead is a peer probing rather than a counter rolling over.
    pub fn new(window: u32) -> Self {
        Self {
            expected: 0,
            buffered: BTreeMap::new(),
            window,
        }
    }

    /// Payloads held out of order.
    pub fn buffered(&self) -> usize {
        self.buffered.len()
    }

    /// The index still being waited for.
    pub fn expected(&self) -> u32 {
        self.expected
    }

    /// Takes one payload and returns everything that became deliverable.
    ///
    /// A payload already delivered yields nothing rather than an error: a duplicate is
    /// what retransmission looks like from here, not misbehaviour.
    pub fn push(&mut self, index: u32, payload: Vec<u8>) -> Result<Vec<Vec<u8>>, OutOfWindow> {
        if index < self.expected {
            return Ok(Vec::new());
        }
        if index.saturating_sub(self.expected) >= self.window {
            return Err(OutOfWindow {
                index,
                expected: self.expected,
            });
        }

        self.buffered.insert(index, payload);

        let mut ready = Vec::new();
        while let Some(next) = self.buffered.remove(&self.expected) {
            ready.push(next);
            self.expected = self.expected.wrapping_add(1);
        }
        Ok(ready)
    }
}

/// Remembers which reliable indices have already been handled.
#[derive(Debug)]
pub struct Dedup {
    seen: BTreeSet<u32>,
    floor: u32,
    window: u32,
}

impl Dedup {
    /// Remembers indices within `window` of the highest one seen.
    pub fn new(window: u32) -> Self {
        Self {
            seen: BTreeSet::new(),
            floor: 0,
            window,
        }
    }

    /// Whether this index is new. Marks it seen when it is.
    pub fn accept(&mut self, index: u32) -> bool {
        if index < self.floor {
            return false;
        }
        if !self.seen.insert(index) {
            return false;
        }

        // Anything older than the window cannot be distinguished from delivered, so it
        // moves into the floor and stops costing a set entry.
        if let Some(&highest) = self.seen.iter().next_back() {
            let new_floor = highest.saturating_sub(self.window);
            if new_floor > self.floor {
                self.floor = new_floor;
                self.seen = self.seen.split_off(&new_floor);
            }
        }
        true
    }

    /// Indices currently remembered.
    pub fn tracked(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(n: u8) -> Vec<u8> {
        vec![n]
    }

    #[test]
    fn in_order_payloads_pass_straight_through() {
        let mut o = Ordering::new(64);
        for n in 0..5u32 {
            let ready = o.push(n, payload(n as u8)).unwrap();
            assert_eq!(ready, vec![payload(n as u8)]);
        }
        assert_eq!(o.buffered(), 0);
    }

    #[test]
    fn a_gap_holds_everything_behind_it() {
        let mut o = Ordering::new(64);
        assert!(o.push(1, payload(1)).unwrap().is_empty());
        assert!(o.push(2, payload(2)).unwrap().is_empty());
        assert_eq!(o.buffered(), 2);

        let ready = o.push(0, payload(0)).unwrap();
        assert_eq!(ready, vec![payload(0), payload(1), payload(2)]);
        assert_eq!(o.buffered(), 0);
        assert_eq!(o.expected(), 3);
    }

    #[test]
    fn a_replayed_payload_is_dropped_not_redelivered() {
        let mut o = Ordering::new(64);
        assert_eq!(o.push(0, payload(0)).unwrap().len(), 1);
        assert!(o.push(0, payload(0)).unwrap().is_empty());
    }

    /// One frame far ahead must not buy unbounded buffering.
    #[test]
    fn an_index_beyond_the_window_is_refused() {
        let mut o = Ordering::new(8);
        assert_eq!(
            o.push(8, payload(0)),
            Err(OutOfWindow {
                index: 8,
                expected: 0
            })
        );
        assert_eq!(o.buffered(), 0);
        assert!(o.push(7, payload(0)).is_ok());
    }

    #[test]
    fn the_window_moves_with_delivery() {
        let mut o = Ordering::new(4);
        assert!(o.push(4, payload(0)).is_err());
        for n in 0..3 {
            o.push(n, payload(0)).unwrap();
        }
        assert!(o.push(4, payload(0)).is_ok(), "window moved forward");
    }

    #[test]
    fn dedup_accepts_once() {
        let mut d = Dedup::new(64);
        assert!(d.accept(1));
        assert!(!d.accept(1));
        assert!(d.accept(2));
    }

    #[test]
    fn dedup_accepts_out_of_order() {
        let mut d = Dedup::new(64);
        assert!(d.accept(5));
        assert!(d.accept(3));
        assert!(!d.accept(5));
    }

    /// The set cannot grow forever: old indices collapse into a floor.
    #[test]
    fn dedup_forgets_beyond_the_window() {
        let mut d = Dedup::new(16);
        for index in 0..1000 {
            assert!(d.accept(index), "{index}");
        }
        assert!(d.tracked() <= 17, "tracked {}", d.tracked());
        assert!(!d.accept(0), "anything below the floor reads as seen");
        assert!(!d.accept(900));
    }
}
