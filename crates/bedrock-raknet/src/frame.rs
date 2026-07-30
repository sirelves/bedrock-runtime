//! Frames: the unit of reliability inside a datagram.
//!
//! ```text
//! u8   reliability in the top 3 bits, split flag at 0x10
//! u16  body length IN BITS, big-endian
//! u24  reliable index      if reliable
//! u24  sequence index      if sequenced
//! u24  order index         if ordered or sequenced
//! u8   order channel       if ordered or sequenced
//! u32  split count         if split
//! u16  split id            if split
//! u32  split index         if split
//! ```
//!
//! The length is in **bits**. A byte count read straight out of that field is eight
//! times too small and desynchronises every frame after it in the datagram.
//!
//! Sources disagree on one conditional: `minecraft.wiki` lists the order index for
//! reliabilities 3 and 7 only, while RakNet's own serialiser writes it for the
//! sequenced ones too. This follows RakNet. Bedrock uses reliable-ordered for
//! essentially everything, so the disagreement is unlikely to show up in traffic —
//! which also means it stays unconfirmed until it does.

use crate::wire::{DecodeError, Reader, Writer};
use std::fmt;

const SPLIT_FLAG: u8 = 0x10;

/// Delivery guarantee for one frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reliability {
    /// Fire and forget.
    Unreliable = 0,
    /// Fire and forget; older frames arriving late are dropped.
    UnreliableSequenced = 1,
    /// Retransmitted until acknowledged.
    Reliable = 2,
    /// Retransmitted and delivered in order. What Bedrock uses for the game.
    ReliableOrdered = 3,
    /// Retransmitted, with older frames dropped rather than reordered.
    ReliableSequenced = 4,
    /// [`Self::Unreliable`], with delivery receipt.
    UnreliableAckReceipt = 5,
    /// [`Self::Reliable`], with delivery receipt.
    ReliableAckReceipt = 6,
    /// [`Self::ReliableOrdered`], with delivery receipt.
    ReliableOrderedAckReceipt = 7,
}

impl Reliability {
    /// Parses the top 3 bits of a frame's flag byte.
    pub fn from_flags(flags: u8) -> Option<Self> {
        Some(match flags >> 5 {
            0 => Self::Unreliable,
            1 => Self::UnreliableSequenced,
            2 => Self::Reliable,
            3 => Self::ReliableOrdered,
            4 => Self::ReliableSequenced,
            5 => Self::UnreliableAckReceipt,
            6 => Self::ReliableAckReceipt,
            7 => Self::ReliableOrderedAckReceipt,
            _ => return None,
        })
    }

    /// Carries a reliable index, and must be acknowledged.
    pub fn is_reliable(self) -> bool {
        matches!(
            self,
            Self::Reliable
                | Self::ReliableOrdered
                | Self::ReliableSequenced
                | Self::ReliableAckReceipt
                | Self::ReliableOrderedAckReceipt
        )
    }

    /// Carries a sequence index.
    pub fn is_sequenced(self) -> bool {
        matches!(self, Self::UnreliableSequenced | Self::ReliableSequenced)
    }

    /// Carries an order index and channel.
    pub fn is_ordered(self) -> bool {
        self.is_sequenced()
            || matches!(
                self,
                Self::ReliableOrdered | Self::ReliableOrderedAckReceipt
            )
    }
}

/// Position of one fragment within a split payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Split {
    /// How many fragments the payload was cut into.
    pub count: u32,
    /// Identifies the payload these fragments belong to.
    pub id: u16,
    /// This fragment's position, from zero.
    pub index: u32,
}

/// One frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// Delivery guarantee.
    pub reliability: Reliability,
    /// Retransmission counter; meaningful when [`Reliability::is_reliable`].
    pub reliable_index: u32,
    /// Sequencing counter; meaningful when [`Reliability::is_sequenced`].
    pub sequence_index: u32,
    /// Ordering counter; meaningful when [`Reliability::is_ordered`].
    pub order_index: u32,
    /// Ordering channel; meaningful when [`Reliability::is_ordered`].
    pub order_channel: u8,
    /// Set when this frame is one fragment of a larger payload.
    pub split: Option<Split>,
    /// The bytes being carried.
    pub payload: Vec<u8>,
}

/// Why a frame did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    /// The buffer ended early.
    Truncated(DecodeError),
    /// The body length was zero.
    EmptyBody,
}

impl From<DecodeError> for FrameError {
    fn from(e: DecodeError) -> Self {
        Self::Truncated(e)
    }
}

impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated(e) => write!(f, "{e}"),
            Self::EmptyBody => write!(f, "frame declares a zero-length body"),
        }
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    /// A frame carrying `payload` with no ordering or splitting applied.
    pub fn new(reliability: Reliability, payload: Vec<u8>) -> Self {
        Self {
            reliability,
            reliable_index: 0,
            sequence_index: 0,
            order_index: 0,
            order_channel: 0,
            split: None,
            payload,
        }
    }

    /// Bytes this frame occupies once encoded.
    pub fn encoded_len(&self) -> usize {
        let mut n = 1 + 2;
        if self.reliability.is_reliable() {
            n += 3;
        }
        if self.reliability.is_sequenced() {
            n += 3;
        }
        if self.reliability.is_ordered() {
            n += 4;
        }
        if self.split.is_some() {
            n += 10;
        }
        n + self.payload.len()
    }

    /// Header bytes for `reliability`, excluding the payload.
    pub fn header_len(reliability: Reliability, split: bool) -> usize {
        let mut n = 1 + 2;
        if reliability.is_reliable() {
            n += 3;
        }
        if reliability.is_sequenced() {
            n += 3;
        }
        if reliability.is_ordered() {
            n += 4;
        }
        if split {
            n += 10;
        }
        n
    }

    /// Appends this frame.
    pub fn encode(&self, w: &mut Writer) {
        let mut flags = (self.reliability as u8) << 5;
        if self.split.is_some() {
            flags |= SPLIT_FLAG;
        }
        w.u8(flags);

        let bits = u16::try_from(self.payload.len().saturating_mul(8)).unwrap_or(u16::MAX);
        w.u16(bits);

        if self.reliability.is_reliable() {
            w.u24(self.reliable_index);
        }
        if self.reliability.is_sequenced() {
            w.u24(self.sequence_index);
        }
        if self.reliability.is_ordered() {
            w.u24(self.order_index).u8(self.order_channel);
        }
        if let Some(split) = self.split {
            w.u32(split.count).u16(split.id).u32(split.index);
        }
        w.bytes(&self.payload);
    }

    /// Reads one frame, leaving the reader positioned at the next.
    pub fn decode(r: &mut Reader<'_>) -> Result<Self, FrameError> {
        let flags = r.u8()?;
        // from_flags covers 0..=7 and the shift cannot produce more.
        let reliability = Reliability::from_flags(flags).unwrap_or(Reliability::Unreliable);
        let is_split = flags & SPLIT_FLAG != 0;

        let bits = usize::from(r.u16()?);
        if bits == 0 {
            return Err(FrameError::EmptyBody);
        }
        let len = bits.div_ceil(8);

        let mut frame = Self {
            reliability,
            reliable_index: 0,
            sequence_index: 0,
            order_index: 0,
            order_channel: 0,
            split: None,
            payload: Vec::new(),
        };

        if reliability.is_reliable() {
            frame.reliable_index = r.u24()?;
        }
        if reliability.is_sequenced() {
            frame.sequence_index = r.u24()?;
        }
        if reliability.is_ordered() {
            frame.order_index = r.u24()?;
            frame.order_channel = r.u8()?;
        }
        if is_split {
            frame.split = Some(Split {
                count: r.u32()?,
                id: r.u16()?,
                index: r.u32()?,
            });
        }

        // Bounded by the u16 bit count, and `bytes` fails before allocating if the
        // buffer is shorter than claimed.
        frame.payload = r.bytes(len)?.to_vec();
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Reliability; 8] = [
        Reliability::Unreliable,
        Reliability::UnreliableSequenced,
        Reliability::Reliable,
        Reliability::ReliableOrdered,
        Reliability::ReliableSequenced,
        Reliability::UnreliableAckReceipt,
        Reliability::ReliableAckReceipt,
        Reliability::ReliableOrderedAckReceipt,
    ];

    fn sample(reliability: Reliability, split: Option<Split>) -> Frame {
        Frame {
            reliability,
            reliable_index: 0x11_2233,
            sequence_index: 0x44_5566,
            order_index: 0x77_8899,
            order_channel: 7,
            split,
            payload: b"hello raknet".to_vec(),
        }
    }

    #[test]
    fn every_reliability_round_trips() {
        for reliability in ALL {
            for split in [
                None,
                Some(Split {
                    count: 3,
                    id: 9,
                    index: 1,
                }),
            ] {
                let frame = sample(reliability, split);
                let mut w = Writer::new();
                frame.encode(&mut w);
                let buf = w.finish();
                assert_eq!(buf.len(), frame.encoded_len(), "{reliability:?}");

                let decoded = Frame::decode(&mut Reader::new(&buf)).unwrap();
                assert_eq!(decoded.reliability, reliability);
                assert_eq!(decoded.payload, frame.payload);
                assert_eq!(decoded.split, split);
                if reliability.is_reliable() {
                    assert_eq!(decoded.reliable_index, frame.reliable_index);
                }
                if reliability.is_sequenced() {
                    assert_eq!(decoded.sequence_index, frame.sequence_index);
                }
                if reliability.is_ordered() {
                    assert_eq!(decoded.order_index, frame.order_index);
                    assert_eq!(decoded.order_channel, 7);
                }
            }
        }
    }

    /// The length field is in bits. Writing bytes there truncates the payload to an
    /// eighth of its size and every following frame reads from the wrong offset.
    #[test]
    fn length_is_written_in_bits() {
        let mut w = Writer::new();
        Frame::new(Reliability::Unreliable, vec![0; 10]).encode(&mut w);
        let buf = w.finish();
        assert_eq!(u16::from_be_bytes([buf[1], buf[2]]), 80);
    }

    #[test]
    fn frames_decode_back_to_back() {
        let mut w = Writer::new();
        Frame::new(Reliability::Unreliable, b"one".to_vec()).encode(&mut w);
        Frame::new(Reliability::ReliableOrdered, b"two".to_vec()).encode(&mut w);
        let buf = w.finish();

        let mut r = Reader::new(&buf);
        assert_eq!(Frame::decode(&mut r).unwrap().payload, b"one");
        assert_eq!(Frame::decode(&mut r).unwrap().payload, b"two");
        assert_eq!(r.remaining(), 0);
    }

    /// A bit count that is not a whole number of bytes rounds up, which is how a
    /// sender that padded to a bit boundary stays decodable.
    #[test]
    fn partial_byte_lengths_round_up() {
        let mut w = Writer::new();
        w.u8(0).u16(9).bytes(&[0xaa, 0xbb]);
        let frame = Frame::decode(&mut Reader::new(&w.finish())).unwrap();
        assert_eq!(frame.payload.len(), 2);
    }

    #[test]
    fn zero_length_body_is_rejected() {
        let mut w = Writer::new();
        w.u8(0).u16(0);
        assert_eq!(
            Frame::decode(&mut Reader::new(&w.finish())),
            Err(FrameError::EmptyBody)
        );
    }

    /// A length larger than the buffer must fail before allocating.
    #[test]
    fn a_lying_length_fails_cleanly() {
        let mut w = Writer::new();
        w.u8(0).u16(u16::MAX).bytes(b"short");
        assert!(matches!(
            Frame::decode(&mut Reader::new(&w.finish())),
            Err(FrameError::Truncated(_))
        ));
    }

    #[test]
    fn truncated_headers_fail_cleanly() {
        let mut w = Writer::new();
        sample(
            Reliability::ReliableOrdered,
            Some(Split {
                count: 2,
                id: 1,
                index: 0,
            }),
        )
        .encode(&mut w);
        let full = w.finish();
        for n in 0..full.len() {
            assert!(Frame::decode(&mut Reader::new(&full[..n])).is_err(), "{n}");
        }
    }

    #[test]
    fn header_len_matches_what_encode_writes() {
        for reliability in ALL {
            for split in [
                None,
                Some(Split {
                    count: 1,
                    id: 0,
                    index: 0,
                }),
            ] {
                let frame = sample(reliability, split);
                assert_eq!(
                    Frame::header_len(reliability, split.is_some()) + frame.payload.len(),
                    frame.encoded_len()
                );
            }
        }
    }
}
