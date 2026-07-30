//! Datagrams: what actually goes in a UDP packet once a connection is open.
//!
//! The first byte decides which of three things it is. Bit `0x80` marks anything in
//! this layer; `0x40` means ACK, `0x20` means NACK, and neither means a frame set.
//!
//! ```text
//! frame set   u8 flags | u24le sequence | frames until the end
//! ack / nack  u8 flags | u16 record count | records
//! record      u8 is_single | u24le start | u24le end   (end omitted when single)
//! ```
//!
//! Acknowledgement ranges are kept as ranges. One record can span sixteen million
//! sequence numbers, so expanding them into a list is a one-record denial of service;
//! the same reason nothing here pre-allocates from the record count.

use crate::frame::{Frame, FrameError};
use crate::wire::{DecodeError, Reader, Writer};
use std::fmt;

const FLAG_VALID: u8 = 0x80;
const FLAG_ACK: u8 = 0x40;
const FLAG_NACK: u8 = 0x20;

/// A run of acknowledged or missing sequence numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Acknowledgement {
    /// Inclusive ranges, in the order the peer sent them.
    pub ranges: Vec<(u32, u32)>,
}

impl Acknowledgement {
    /// One range covering a single sequence number.
    pub fn single(sequence: u32) -> Self {
        Self {
            ranges: vec![(sequence, sequence)],
        }
    }

    /// Whether `sequence` falls in any range.
    pub fn contains(&self, sequence: u32) -> bool {
        self.ranges
            .iter()
            .any(|&(start, end)| sequence >= start && sequence <= end)
    }

    fn encode(&self, w: &mut Writer, flags: u8) {
        w.u8(flags);
        w.u16(u16::try_from(self.ranges.len()).unwrap_or(u16::MAX));
        for &(start, end) in self.ranges.iter().take(usize::from(u16::MAX)) {
            if start == end {
                w.u8(1).u24(start);
            } else {
                w.u8(0).u24(start).u24(end);
            }
        }
    }

    fn decode(r: &mut Reader<'_>) -> Result<Self, DatagramError> {
        let count = r.u16()?;
        let mut ranges = Vec::new();
        for _ in 0..count {
            let start = if r.u8()? == 1 {
                let single = r.u24()?;
                ranges.push((single, single));
                continue;
            } else {
                r.u24()?
            };
            ranges.push((start, r.u24()?));
        }
        Ok(Self { ranges })
    }
}

/// A sequenced datagram carrying frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameSet {
    /// Datagram sequence number, used to build ACK and NACK ranges.
    pub sequence: u32,
    /// Frames packed into this datagram.
    pub frames: Vec<Frame>,
}

/// One decoded datagram.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Datagram {
    /// Frames, with a sequence number to acknowledge.
    FrameSet(FrameSet),
    /// The peer received these.
    Ack(Acknowledgement),
    /// The peer is missing these.
    Nack(Acknowledgement),
}

/// Why a datagram did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatagramError {
    /// The buffer ended early.
    Truncated(DecodeError),
    /// The high bit was clear, so this is not connected-phase traffic.
    NotConnected(u8),
    /// A frame inside the datagram did not decode.
    Frame(FrameError),
}

impl From<DecodeError> for DatagramError {
    fn from(e: DecodeError) -> Self {
        Self::Truncated(e)
    }
}

impl From<FrameError> for DatagramError {
    fn from(e: FrameError) -> Self {
        Self::Frame(e)
    }
}

impl fmt::Display for DatagramError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(e) => write!(f, "{e}"),
            Self::NotConnected(flags) => {
                write!(f, "first byte {flags:#04x} is not a connected datagram")
            }
            Self::Frame(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DatagramError {}

impl Datagram {
    /// Appends this datagram.
    pub fn encode(&self, w: &mut Writer) {
        match self {
            Self::FrameSet(set) => {
                w.u8(FLAG_VALID).u24(set.sequence);
                for frame in &set.frames {
                    frame.encode(w);
                }
            }
            Self::Ack(ack) => ack.encode(w, FLAG_VALID | FLAG_ACK),
            Self::Nack(nack) => nack.encode(w, FLAG_VALID | FLAG_NACK),
        }
    }

    /// Decodes a whole UDP payload.
    pub fn decode(buf: &[u8]) -> Result<Self, DatagramError> {
        let mut r = Reader::new(buf);
        let flags = r.u8()?;
        if flags & FLAG_VALID == 0 {
            return Err(DatagramError::NotConnected(flags));
        }

        if flags & FLAG_ACK != 0 {
            return Ok(Self::Ack(Acknowledgement::decode(&mut r)?));
        }
        if flags & FLAG_NACK != 0 {
            return Ok(Self::Nack(Acknowledgement::decode(&mut r)?));
        }

        let sequence = r.u24()?;
        let mut frames = Vec::new();
        while r.remaining() > 0 {
            frames.push(Frame::decode(&mut r)?);
        }
        Ok(Self::FrameSet(FrameSet { sequence, frames }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Reliability;

    fn round_trip(datagram: &Datagram) -> Datagram {
        let mut w = Writer::new();
        datagram.encode(&mut w);
        Datagram::decode(&w.finish()).unwrap()
    }

    #[test]
    fn frame_set_round_trips() {
        let datagram = Datagram::FrameSet(FrameSet {
            sequence: 0x00_1234,
            frames: vec![
                Frame::new(Reliability::Unreliable, b"one".to_vec()),
                Frame::new(Reliability::ReliableOrdered, b"two".to_vec()),
            ],
        });
        assert_eq!(round_trip(&datagram), datagram);
    }

    #[test]
    fn ack_and_nack_are_distinguished() {
        let ranges = Acknowledgement {
            ranges: vec![(1, 1), (5, 9)],
        };
        assert_eq!(
            round_trip(&Datagram::Ack(ranges.clone())),
            Datagram::Ack(ranges.clone())
        );
        assert_eq!(
            round_trip(&Datagram::Nack(ranges.clone())),
            Datagram::Nack(ranges)
        );
    }

    #[test]
    fn single_records_are_shorter_than_ranges() {
        let mut single = Writer::new();
        Datagram::Ack(Acknowledgement::single(7)).encode(&mut single);
        let mut range = Writer::new();
        Datagram::Ack(Acknowledgement {
            ranges: vec![(7, 8)],
        })
        .encode(&mut range);
        assert_eq!(single.len() + 3, range.len());
    }

    #[test]
    fn contains_covers_the_whole_range() {
        let ack = Acknowledgement {
            ranges: vec![(10, 12), (20, 20)],
        };
        for seq in [10, 11, 12, 20] {
            assert!(ack.contains(seq), "{seq}");
        }
        for seq in [9, 13, 19, 21] {
            assert!(!ack.contains(seq), "{seq}");
        }
    }

    /// One record can claim sixteen million sequence numbers. It stays one range.
    #[test]
    fn a_huge_range_stays_one_range() {
        let mut w = Writer::new();
        w.u8(FLAG_VALID | FLAG_ACK)
            .u16(1)
            .u8(0)
            .u24(0)
            .u24(0xff_ffff);
        let Datagram::Ack(ack) = Datagram::decode(&w.finish()).unwrap() else {
            unreachable!("encoded as an ack")
        };
        assert_eq!(ack.ranges, vec![(0, 0xff_ffff)]);
        assert!(ack.contains(8_000_000));
    }

    /// A record count far beyond what the datagram holds must fail on the short read
    /// rather than reserve for it.
    #[test]
    fn a_lying_record_count_fails_cleanly() {
        let mut w = Writer::new();
        w.u8(FLAG_VALID | FLAG_ACK).u16(u16::MAX).u8(1).u24(3);
        assert!(matches!(
            Datagram::decode(&w.finish()),
            Err(DatagramError::Truncated(_))
        ));
    }

    #[test]
    fn offline_traffic_is_not_mistaken_for_a_datagram() {
        assert_eq!(
            Datagram::decode(&[0x01, 0, 0, 0]),
            Err(DatagramError::NotConnected(0x01))
        );
    }

    #[test]
    fn empty_input_fails_cleanly() {
        assert!(matches!(
            Datagram::decode(&[]),
            Err(DatagramError::Truncated(_))
        ));
    }

    #[test]
    fn a_broken_frame_fails_the_datagram() {
        let mut w = Writer::new();
        w.u8(FLAG_VALID).u24(1).u8(0).u16(0);
        assert!(matches!(
            Datagram::decode(&w.finish()),
            Err(DatagramError::Frame(_))
        ));
    }

    #[test]
    fn truncated_prefixes_never_panic() {
        let datagram = Datagram::FrameSet(FrameSet {
            sequence: 9,
            frames: vec![Frame::new(
                Reliability::ReliableOrdered,
                b"payload".to_vec(),
            )],
        });
        let mut w = Writer::new();
        datagram.encode(&mut w);
        let full = w.finish();
        for n in 0..full.len() {
            let _ = Datagram::decode(&full[..n]);
        }
    }
}
