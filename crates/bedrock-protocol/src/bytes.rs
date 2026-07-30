//! Reading and writing the primitives the Bedrock game protocol uses.
//!
//! Little-endian, unlike RakNet underneath it, with varints for lengths and packet
//! ids. The one big-endian field met so far is `ClientNetworkVersion`, which Mojang's
//! own schema marks as an exception — so [`Reader::u32_be`] exists and is the odd one
//! out, not the rule.
//!
//! A separate cursor from `bedrock-raknet`'s on purpose: the endianness is opposite and
//! the crates may not depend on each other. Sharing one would mean a type that is
//! big-endian in one caller and little-endian in the other.
//!
//! Every read is bounds-checked and none allocates from a length off the wire.

use std::fmt;

/// Longest a varint may be before it is a peer wasting our time: five bytes carry the
/// 32 bits, and anything longer is padding a value that cannot fit.
const MAX_VARINT_BYTES: usize = 5;

/// Why a decode failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// The buffer ended early.
    UnexpectedEnd {
        /// Bytes wanted.
        needed: usize,
        /// Bytes left.
        available: usize,
    },
    /// A varint ran past the bytes a 32-bit value can occupy.
    VarintTooLong,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedEnd { needed, available } => {
                write!(f, "needed {needed} byte(s), {available} available")
            }
            Self::VarintTooLong => write!(f, "varint longer than {MAX_VARINT_BYTES} bytes"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Result of a decode step.
pub type Result<T> = std::result::Result<T, DecodeError>;

/// A cursor over a packet.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Starts at the beginning of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// Bytes not yet read.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Whether everything has been read.
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Borrows the next `n` bytes.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(DecodeError::UnexpectedEnd {
            needed: n,
            available: self.remaining(),
        })?;
        let slice = self
            .buf
            .get(self.pos..end)
            .ok_or(DecodeError::UnexpectedEnd {
                needed: n,
                available: self.remaining(),
            })?;
        self.pos = end;
        Ok(slice)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut out = [0u8; N];
        out.copy_from_slice(self.bytes(N)?);
        Ok(out)
    }

    /// Reads one byte.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    /// Reads a little-endian `u16`.
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }

    /// Reads a little-endian `f32`.
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }

    /// Reads a big-endian `u32`, for the fields Mojang marks as such.
    pub fn u32_be(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(self.array()?))
    }

    /// Reads an unsigned varint.
    pub fn varint(&mut self) -> Result<u32> {
        let mut value: u32 = 0;
        for byte_index in 0..MAX_VARINT_BYTES {
            let byte = self.u8()?;
            value |= u32::from(byte & 0x7f) << (byte_index * 7);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(DecodeError::VarintTooLong)
    }

    /// Reads a varint length followed by that many bytes.
    pub fn prefixed(&mut self) -> Result<&'a [u8]> {
        let len = self.varint()? as usize;
        self.bytes(len)
    }
}

/// A growable packet buffer.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// An empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one byte.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    /// Appends a little-endian `u16`.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends a little-endian `f32`.
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_le_bytes());
        self
    }

    /// Appends a big-endian `u32`.
    pub fn u32_be(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Appends an unsigned varint.
    pub fn varint(&mut self, mut v: u32) -> &mut Self {
        loop {
            let byte = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.buf.push(byte);
                return self;
            }
            self.buf.push(byte | 0x80);
        }
    }

    /// Appends raw bytes.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    /// Appends a varint length followed by the bytes.
    pub fn prefixed(&mut self, v: &[u8]) -> &mut Self {
        self.varint(u32::try_from(v.len()).unwrap_or(u32::MAX));
        self.bytes(v)
    }

    /// Bytes written so far.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing has been written.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Takes the buffer.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip() {
        for value in [0, 1, 127, 128, 300, 16_383, 16_384, 193, u32::MAX] {
            let mut w = Writer::new();
            w.varint(value);
            let buf = w.finish();
            let mut r = Reader::new(&buf);
            assert_eq!(r.varint().unwrap(), value, "{value}");
            assert!(r.is_empty());
        }
    }

    /// The exact bytes a real client sent for packet id 193.
    #[test]
    fn varint_193_matches_the_capture() {
        let mut w = Writer::new();
        w.varint(193);
        assert_eq!(w.finish(), vec![0xc1, 0x01]);
    }

    #[test]
    fn varints_use_the_fewest_bytes() {
        for (value, len) in [(0u32, 1), (127, 1), (128, 2), (16_383, 2), (16_384, 3)] {
            let mut w = Writer::new();
            w.varint(value);
            assert_eq!(w.len(), len, "{value}");
        }
    }

    /// A peer padding a varint forever must not spin the decoder.
    #[test]
    fn an_endless_varint_is_refused() {
        let buf = [0x80u8; 32];
        assert_eq!(Reader::new(&buf).varint(), Err(DecodeError::VarintTooLong));
    }

    #[test]
    fn a_truncated_varint_fails_cleanly() {
        let buf = [0x80u8, 0x80];
        assert!(matches!(
            Reader::new(&buf).varint(),
            Err(DecodeError::UnexpectedEnd { .. })
        ));
    }

    #[test]
    fn little_endian_is_the_default() {
        let mut w = Writer::new();
        w.u16(0x1234);
        assert_eq!(w.finish(), vec![0x34, 0x12]);
    }

    /// The exception Mojang's schema marks, and the bytes our capture carried.
    #[test]
    fn client_network_version_is_big_endian() {
        let mut w = Writer::new();
        w.u32_be(1001);
        assert_eq!(w.finish(), vec![0x00, 0x00, 0x03, 0xe9]);
    }

    #[test]
    fn prefixed_slices_round_trip() {
        let mut w = Writer::new();
        w.prefixed(b"hello").prefixed(b"");
        let buf = w.finish();
        let mut r = Reader::new(&buf);
        assert_eq!(r.prefixed().unwrap(), b"hello");
        assert_eq!(r.prefixed().unwrap(), b"");
        assert!(r.is_empty());
    }

    #[test]
    fn a_prefix_longer_than_the_buffer_fails_cleanly() {
        let mut w = Writer::new();
        w.varint(9999).bytes(b"short");
        assert!(Reader::new(&w.finish()).prefixed().is_err());
    }

    #[test]
    fn floats_round_trip() {
        let mut w = Writer::new();
        w.f32(0.5);
        assert!((Reader::new(&w.finish()).f32().unwrap() - 0.5).abs() < f32::EPSILON);
    }
}
