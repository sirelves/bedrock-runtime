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

    /// Appends raw bytes.
    pub fn bytes(&mut self, v: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(v);
        self
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
