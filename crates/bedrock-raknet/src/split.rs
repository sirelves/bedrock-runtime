//! Splitting payloads that do not fit in a datagram, and putting them back together.
//!
//! Reassembly is the most exposed allocation in the crate: a peer states how many
//! fragments are coming and we hold what has arrived until the rest does. Every bound
//! here exists because of a specific way that goes wrong — see [`Limits`] and
//! `SECURITY.md`.

use crate::datagram::DATAGRAM_HEADER_LEN;
use crate::frame::{Frame, Reliability, Split};
use std::collections::BTreeMap;
use std::fmt;
use std::time::{Duration, Instant};

/// Largest payload one frame can declare, since the length field is a `u16` of bits.
pub const MAX_FRAME_PAYLOAD: usize = u16::MAX as usize / 8;

/// Bytes available for a fragment's payload inside a datagram of `payload_limit` bytes.
pub fn fragment_capacity(payload_limit: usize) -> usize {
    payload_limit
        .saturating_sub(DATAGRAM_HEADER_LEN)
        .saturating_sub(Frame::header_len(Reliability::ReliableOrdered, true))
        .min(MAX_FRAME_PAYLOAD)
}

/// Cuts a payload into fragments and hands out split ids.
#[derive(Debug, Default)]
pub struct Splitter {
    next_id: u16,
}

impl Splitter {
    /// Splits `payload` into fragments of at most `capacity` bytes each.
    ///
    /// Returns a single unsplit frame when it already fits, so callers do not have to
    /// special-case small payloads. Yields nothing if `capacity` is zero.
    pub fn split(&mut self, payload: Vec<u8>, capacity: usize) -> Vec<Frame> {
        if capacity == 0 {
            return Vec::new();
        }
        if payload.len() <= capacity {
            return vec![Frame::new(Reliability::ReliableOrdered, payload)];
        }

        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);

        let chunks: Vec<&[u8]> = payload.chunks(capacity).collect();
        let count = u32::try_from(chunks.len()).unwrap_or(u32::MAX);

        chunks
            .into_iter()
            .enumerate()
            .map(|(index, chunk)| Frame {
                reliability: Reliability::ReliableOrdered,
                reliable_index: 0,
                sequence_index: 0,
                order_index: 0,
                order_channel: 0,
                split: Some(Split {
                    count,
                    id,
                    index: u32::try_from(index).unwrap_or(u32::MAX),
                }),
                payload: chunk.to_vec(),
            })
            .collect()
    }
}

/// What a session will spend on reassembly before giving up on a peer.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Largest payload that may be reassembled.
    pub max_payload: usize,
    /// Largest fragment count a peer may announce. Without it, a peer announces four
    /// billion fragments and we would size a buffer from that number.
    pub max_fragments: u32,
    /// Concurrent split ids. Without it, a peer opens one reassembly per id and never
    /// finishes any of them.
    pub max_pending: usize,
    /// How long an incomplete reassembly survives. Without it, the fragments a peer
    /// never completes are held forever.
    pub timeout: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_payload: 4 * 1024 * 1024,
            max_fragments: 4096,
            max_pending: 8,
            timeout: Duration::from_secs(10),
        }
    }
}

/// Why a fragment was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitError {
    /// A count of zero, or an index at or past the count.
    Inconsistent {
        /// Announced fragment count.
        count: u32,
        /// Announced index.
        index: u32,
    },
    /// The announced fragment count is beyond [`Limits::max_fragments`].
    TooManyFragments(u32),
    /// The payload would grow past [`Limits::max_payload`].
    PayloadTooLarge,
    /// Too many reassemblies already open.
    TooManyPending,
    /// The same index arrived twice with different content.
    DuplicateIndex(u32),
}

impl fmt::Display for SplitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inconsistent { count, index } => {
                write!(f, "fragment {index} of {count} is not a valid position")
            }
            Self::TooManyFragments(count) => write!(f, "peer announced {count} fragments"),
            Self::PayloadTooLarge => write!(f, "reassembled payload would exceed the limit"),
            Self::TooManyPending => write!(f, "too many reassemblies already open"),
            Self::DuplicateIndex(index) => write!(f, "fragment {index} arrived twice"),
        }
    }
}

impl std::error::Error for SplitError {}

#[derive(Debug)]
struct Pending {
    count: u32,
    fragments: BTreeMap<u32, Vec<u8>>,
    bytes: usize,
    first_seen: Instant,
}

/// Collects fragments until a payload is whole.
#[derive(Debug)]
pub struct Reassembler {
    limits: Limits,
    pending: BTreeMap<u16, Pending>,
    buffered: usize,
}

impl Reassembler {
    /// A reassembler with the given limits.
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            pending: BTreeMap::new(),
            buffered: 0,
        }
    }

    /// Bytes currently held across all open reassemblies.
    pub fn buffered(&self) -> usize {
        self.buffered
    }

    /// Open reassemblies.
    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Drops reassemblies that have been open longer than [`Limits::timeout`].
    ///
    /// Takes `now` rather than reading the clock so a test can move time without
    /// sleeping.
    pub fn expire(&mut self, now: Instant) {
        let timeout = self.limits.timeout;
        let buffered = &mut self.buffered;
        self.pending.retain(|_, entry| {
            let alive = now.duration_since(entry.first_seen) < timeout;
            if !alive {
                *buffered -= entry.bytes;
            }
            alive
        });
    }

    /// Feeds one fragment in. Returns the payload once the last one arrives.
    ///
    /// A frame with no split information is returned as-is: an unsplit payload is a
    /// one-fragment payload, and callers should not have to branch on it.
    pub fn push(&mut self, frame: Frame, now: Instant) -> Result<Option<Vec<u8>>, SplitError> {
        let Some(split) = frame.split else {
            return Ok(Some(frame.payload));
        };

        if split.count == 0 || split.index >= split.count {
            return Err(SplitError::Inconsistent {
                count: split.count,
                index: split.index,
            });
        }
        if split.count > self.limits.max_fragments {
            return Err(SplitError::TooManyFragments(split.count));
        }

        let is_new = !self.pending.contains_key(&split.id);
        if is_new && self.pending.len() >= self.limits.max_pending {
            return Err(SplitError::TooManyPending);
        }
        if self.buffered + frame.payload.len() > self.limits.max_payload {
            return Err(SplitError::PayloadTooLarge);
        }

        let entry = self.pending.entry(split.id).or_insert_with(|| Pending {
            count: split.count,
            fragments: BTreeMap::new(),
            bytes: 0,
            first_seen: now,
        });

        // A peer that changes its mind about the count mid-payload is either broken or
        // probing; the earlier count is the one we sized against.
        if entry.count != split.count {
            return Err(SplitError::Inconsistent {
                count: split.count,
                index: split.index,
            });
        }
        if entry.fragments.contains_key(&split.index) {
            return Err(SplitError::DuplicateIndex(split.index));
        }

        entry.bytes += frame.payload.len();
        self.buffered += frame.payload.len();
        entry.fragments.insert(split.index, frame.payload);

        if u32::try_from(entry.fragments.len()).unwrap_or(u32::MAX) < entry.count {
            return Ok(None);
        }

        let Some(entry) = self.pending.remove(&split.id) else {
            return Ok(None);
        };
        self.buffered -= entry.bytes;

        let mut payload = Vec::with_capacity(entry.bytes);
        for fragment in entry.fragments.into_values() {
            payload.extend_from_slice(&fragment);
        }
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    fn reassemble(frames: Vec<Frame>) -> Option<Vec<u8>> {
        let mut r = Reassembler::new(Limits::default());
        let t = now();
        let mut out = None;
        for frame in frames {
            if let Some(payload) = r.push(frame, t).unwrap() {
                out = Some(payload);
            }
        }
        assert_eq!(r.buffered(), 0, "nothing should stay buffered");
        out
    }

    /// The M0.2 criterion: 1 KiB, 64 KiB and 1 MiB survive a round trip. 1 KiB fits in
    /// one fragment at a real MTU and is expected not to split.
    #[test]
    fn payloads_round_trip_through_fragmentation() {
        let capacity = fragment_capacity(1472);
        for size in [1024usize, 64 * 1024, 1024 * 1024] {
            let payload: Vec<u8> = (0..size).map(|i| (i % 251) as u8).collect();
            let frames = Splitter::default().split(payload.clone(), capacity);
            assert_eq!(frames.len(), size.div_ceil(capacity), "{size}");
            assert_eq!(reassemble(frames), Some(payload), "{size}");
        }
    }

    #[test]
    fn a_payload_that_fits_is_not_split() {
        let frames = Splitter::default().split(b"small".to_vec(), 1400);
        assert_eq!(frames.len(), 1);
        assert!(frames[0].split.is_none());
        assert_eq!(reassemble(frames), Some(b"small".to_vec()));
    }

    #[test]
    fn fragments_reassemble_out_of_order() {
        let payload: Vec<u8> = (0..5000).map(|i| (i % 253) as u8).collect();
        let mut frames = Splitter::default().split(payload.clone(), 1000);
        frames.reverse();
        assert_eq!(reassemble(frames), Some(payload));
    }

    #[test]
    fn every_fragment_fits_the_datagram() {
        let limit = 1472;
        let capacity = fragment_capacity(limit);
        for frame in Splitter::default().split(vec![0; 100_000], capacity) {
            assert!(
                frame.encoded_len() + DATAGRAM_HEADER_LEN <= limit,
                "{} bytes overruns {limit}",
                frame.encoded_len()
            );
        }
    }

    #[test]
    fn split_ids_differ_between_payloads() {
        let mut splitter = Splitter::default();
        let a = splitter.split(vec![0; 3000], 1000);
        let b = splitter.split(vec![0; 3000], 1000);
        assert_ne!(a[0].split.map(|s| s.id), b[0].split.map(|s| s.id));
    }

    fn fragment(id: u16, count: u32, index: u32, len: usize) -> Frame {
        Frame {
            reliability: Reliability::ReliableOrdered,
            reliable_index: 0,
            sequence_index: 0,
            order_index: 0,
            order_channel: 0,
            split: Some(Split { count, id, index }),
            payload: vec![0; len],
        }
    }

    /// Announce four billion fragments, send one, and nothing is sized from it.
    #[test]
    fn an_absurd_fragment_count_is_refused() {
        let mut r = Reassembler::new(Limits::default());
        assert_eq!(
            r.push(fragment(1, u32::MAX, 0, 10), now()),
            Err(SplitError::TooManyFragments(u32::MAX))
        );
        assert_eq!(r.buffered(), 0);
    }

    #[test]
    fn an_index_past_the_count_is_refused() {
        let mut r = Reassembler::new(Limits::default());
        assert!(matches!(
            r.push(fragment(1, 4, 4, 10), now()),
            Err(SplitError::Inconsistent { .. })
        ));
        assert!(matches!(
            r.push(fragment(1, 0, 0, 10), now()),
            Err(SplitError::Inconsistent { .. })
        ));
    }

    /// One fragment per split id, never completed, is how a peer drives memory up.
    #[test]
    fn too_many_open_reassemblies_are_refused() {
        let limits = Limits {
            max_pending: 3,
            ..Limits::default()
        };
        let mut r = Reassembler::new(limits);
        let t = now();
        for id in 0..3 {
            assert_eq!(r.push(fragment(id, 10, 0, 100), t), Ok(None));
        }
        assert_eq!(
            r.push(fragment(99, 10, 0, 100), t),
            Err(SplitError::TooManyPending)
        );
        assert_eq!(r.pending(), 3);
    }

    #[test]
    fn buffering_past_the_limit_is_refused() {
        let limits = Limits {
            max_payload: 250,
            ..Limits::default()
        };
        let mut r = Reassembler::new(limits);
        let t = now();
        assert_eq!(r.push(fragment(1, 4, 0, 200), t), Ok(None));
        assert_eq!(
            r.push(fragment(1, 4, 1, 200), t),
            Err(SplitError::PayloadTooLarge)
        );
    }

    #[test]
    fn a_repeated_index_is_refused_without_growing() {
        let mut r = Reassembler::new(Limits::default());
        let t = now();
        assert_eq!(r.push(fragment(1, 4, 0, 100), t), Ok(None));
        assert_eq!(
            r.push(fragment(1, 4, 0, 100), t),
            Err(SplitError::DuplicateIndex(0))
        );
        assert_eq!(r.buffered(), 100);
    }

    #[test]
    fn a_changed_count_is_refused() {
        let mut r = Reassembler::new(Limits::default());
        let t = now();
        assert_eq!(r.push(fragment(1, 4, 0, 100), t), Ok(None));
        assert!(matches!(
            r.push(fragment(1, 9, 1, 100), t),
            Err(SplitError::Inconsistent { .. })
        ));
    }

    #[test]
    fn incomplete_reassemblies_expire() {
        let limits = Limits {
            timeout: Duration::from_secs(10),
            ..Limits::default()
        };
        let mut r = Reassembler::new(limits);
        let t = now();
        assert_eq!(r.push(fragment(1, 4, 0, 100), t), Ok(None));
        assert_eq!(r.buffered(), 100);

        r.expire(t + Duration::from_secs(5));
        assert_eq!(r.pending(), 1, "still within the timeout");

        r.expire(t + Duration::from_secs(11));
        assert_eq!(r.pending(), 0);
        assert_eq!(r.buffered(), 0, "expiring must give the memory back");
    }
}
