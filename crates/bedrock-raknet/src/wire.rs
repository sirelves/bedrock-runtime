//! Reading and writing the primitive types RakNet puts on the wire.
//!
//! RakNet is big-endian for everything except datagram sequence numbers, which are
//! 24-bit little-endian.
//!
//! Every read is bounds-checked and returns [`DecodeError`] instead of panicking, and
//! no read allocates: [`Reader::bytes`] hands back a borrowed slice. That is a
//! requirement, not a style choice — this module is the first thing an unauthenticated
//! attacker reaches. See `SECURITY.md`.

use std::fmt;

/// A read that ran past the end of the buffer.
///
/// Carries what was asked for and what was left, because "unexpected end of input" with
/// no numbers is useless when a capture does not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeError {
    /// How many bytes the read wanted.
    pub needed: usize,
    /// How many bytes were actually left.
    pub available: usize,
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "unexpected end of input: needed {} byte(s), {} available",
            self.needed, self.available
        )
    }
}

impl std::error::Error for DecodeError {}

/// Result of a decode step.
pub type Result<T> = std::result::Result<T, DecodeError>;

/// A cursor over a received datagram.
#[derive(Debug, Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    /// Starts reading at the beginning of `buf`.
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    /// How many bytes are still unread.
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// Borrows the next `n` bytes.
    ///
    /// Returns a slice rather than a `Vec` so that a length field coming off the wire
    /// can never drive an allocation.
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).ok_or(DecodeError {
            needed: n,
            available: self.remaining(),
        })?;
        let slice = self.buf.get(self.pos..end).ok_or(DecodeError {
            needed: n,
            available: self.remaining(),
        })?;
        self.pos = end;
        Ok(slice)
    }

    /// Borrows the next `N` bytes as a fixed-size array.
    pub fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.bytes(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    /// Reads one byte.
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    /// Reads a big-endian `u16`.
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(self.array()?))
    }

    /// Reads a big-endian `i64`.
    pub fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_be_bytes(self.array()?))
    }

    /// Reads a 24-bit **little-endian** integer.
    ///
    /// The one place RakNet flips endianness: datagram sequence numbers and the
    /// reliability indices are 24-bit little-endian while everything around them is
    /// big-endian. Reading these as big-endian produces numbers that look almost
    /// plausible, which is worse than numbers that look wrong.
    pub fn u24(&mut self) -> Result<u32> {
        let b = self.array::<3>()?;
        Ok(u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16)
    }
}

/// A growable buffer for building a datagram.
#[derive(Debug, Default, Clone)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    /// Starts an empty datagram.
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends one byte.
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }

    /// Appends a big-endian `i64`.
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Appends a big-endian `u16`.
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Appends a 24-bit **little-endian** integer.
    ///
    /// Only the low 24 bits are written. That is not truncation papering over a bug:
    /// RakNet sequence numbers are 24-bit and wrap at 2^24 by design, so the caller
    /// counting past that is expected.
    pub fn u24(&mut self, v: u32) -> &mut Self {
        self.buf
            .extend_from_slice(&[v as u8, (v >> 8) as u8, (v >> 16) as u8]);
        self
    }

    /// Appends raw bytes.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
    }

    /// Appends `count` zero bytes.
    ///
    /// Used to pad `OpenConnectionRequest1` up to the MTU being probed — the size of
    /// that datagram *is* the question being asked.
    pub fn zeros(&mut self, count: usize) -> &mut Self {
        self.buf.resize(self.buf.len() + count, 0);
        self
    }

    /// How many bytes have been written so far.
    pub fn len(&self) -> usize {
        self.buf.len()
    }

    /// Whether nothing has been written yet.
    pub fn is_empty(&self) -> bool {
        self.buf.is_empty()
    }

    /// Consumes the writer and yields the datagram.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_big_endian() {
        let mut r = Reader::new(&[0x01, 0x12, 0x34, 0, 0, 0, 0, 0, 0, 0, 0x2a]);
        assert_eq!(r.u8().unwrap(), 0x01);
        assert_eq!(r.u16().unwrap(), 0x1234);
        assert_eq!(r.i64().unwrap(), 42);
        assert_eq!(r.remaining(), 0);
    }

    #[test]
    fn short_read_reports_both_numbers() {
        let mut r = Reader::new(&[0x00, 0x01]);
        assert_eq!(
            r.i64(),
            Err(DecodeError {
                needed: 8,
                available: 2
            })
        );
    }

    #[test]
    fn short_read_does_not_consume() {
        let mut r = Reader::new(&[0xaa, 0xbb]);
        assert!(r.bytes(9).is_err());
        assert_eq!(
            r.remaining(),
            2,
            "a failed read must not advance the cursor"
        );
        assert_eq!(r.u8().unwrap(), 0xaa);
    }

    /// A length field is attacker-controlled, so a huge one must fail cleanly rather
    /// than overflow the cursor arithmetic. See `SECURITY.md`.
    #[test]
    fn huge_length_does_not_overflow() {
        let mut r = Reader::new(&[0u8; 4]);
        assert!(r.bytes(usize::MAX).is_err());
    }

    #[test]
    fn u24_is_little_endian() {
        // 0x00563412 little-endian is 12 34 56 — the same bytes read big-endian would
        // be 0x123456, so this test fails loudly if the endianness ever flips.
        let mut r = Reader::new(&[0x12, 0x34, 0x56]);
        assert_eq!(r.u24().unwrap(), 0x0056_3412);
    }

    #[test]
    fn u24_round_trips_across_its_whole_range() {
        for v in [0, 1, 255, 256, 0xff_ffff] {
            let mut w = Writer::new();
            w.u24(v);
            let buf = w.finish();
            assert_eq!(buf.len(), 3);
            assert_eq!(Reader::new(&buf).u24().unwrap(), v, "value {v}");
        }
    }

    /// Sequence numbers wrap at 2^24; writing past it keeps the low bits rather than
    /// failing, because wrapping is the protocol's behaviour and not a caller error.
    #[test]
    fn u24_keeps_the_low_bits() {
        let mut w = Writer::new();
        w.u24(0x0100_0001);
        assert_eq!(Reader::new(&w.finish()).u24().unwrap(), 1);
    }

    #[test]
    fn u24_short_read_fails() {
        assert!(Reader::new(&[0x01, 0x02]).u24().is_err());
    }

    #[test]
    fn zeros_pads_to_a_target_size() {
        let mut w = Writer::new();
        w.u8(0x05).zeros(10);
        assert_eq!(w.len(), 11);
        assert_eq!(w.finish()[1..], [0u8; 10]);
    }

    #[test]
    fn writer_round_trips_through_reader() {
        let mut w = Writer::new();
        w.u8(0x1c).i64(-1).bytes(&[0xde, 0xad]);
        let buf = w.finish();

        let mut r = Reader::new(&buf);
        assert_eq!(r.u8().unwrap(), 0x1c);
        assert_eq!(r.i64().unwrap(), -1);
        assert_eq!(r.bytes(2).unwrap(), &[0xde, 0xad]);
    }
}
