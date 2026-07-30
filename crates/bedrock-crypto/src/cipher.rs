//! The encrypted packet stream.
//!
//! AES-256 in CFB8, one continuous cipher state per direction, with an 8-byte checksum
//! appended to each packet before encryption:
//!
//! ```text
//! plaintext || SHA-256(counter_le || plaintext || key)[..8]
//! ```
//!
//! The counter starts at zero and increments per packet, separately for each direction.
//! It is never sent — both sides count — so a lost or reordered packet desynchronises
//! the stream permanently. That is fine here: the transport underneath already
//! guarantees reliable ordered delivery.
//!
//! # Why CFB8 and not GCM
//!
//! Measured, not chosen. A real client answered our handshake with twelve bytes: the
//! batch marker and eleven more. An empty `ClientToServerHandshake` is three bytes of
//! plaintext plus the eight-byte checksum — eleven. A length-preserving mode produces
//! exactly that; GCM would have added a sixteen-byte tag.
//!
//! # Still unconfirmed
//!
//! That the IV is the key's first sixteen bytes, and that the checksum is built in this
//! order. Both fail the same way — the checksum mismatches — and the first packet that
//! decrypts cleanly settles them.

use crate::agreement::SessionKey;
use aes::Aes256;
use aes::cipher::{Block, BlockDecryptMut, BlockEncryptMut, KeyIvInit};
use ring::digest;
use std::fmt;
use subtle::ConstantTimeEq;

type Encryptor = cfb8::Encryptor<Aes256>;
type Decryptor = cfb8::Decryptor<Aes256>;

/// Bytes of checksum appended to every packet.
pub const CHECKSUM_LEN: usize = 8;

/// Why a packet did not decrypt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherError {
    /// Shorter than the checksum it must carry.
    TooShort,
    /// The checksum did not match — a corrupted packet, a desynchronised counter, or
    /// the wrong key.
    BadChecksum,
}

impl fmt::Display for CipherError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort => write!(f, "packet is shorter than its checksum"),
            Self::BadChecksum => write!(f, "checksum did not match"),
        }
    }
}

impl std::error::Error for CipherError {}

/// One session's encrypted stream, both directions.
pub struct Cipher {
    encryptor: Encryptor,
    decryptor: Decryptor,
    key: [u8; 32],
    sent: u64,
    received: u64,
}

impl fmt::Debug for Cipher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Cipher(sent: {}, received: {})",
            self.sent, self.received
        )
    }
}

impl Cipher {
    /// Starts a stream from an agreed session key.
    ///
    /// The IV is the key's first sixteen bytes: there is no separate IV on the wire,
    /// and both sides have to arrive at the same one from the same material.
    pub fn new(key: &SessionKey) -> Self {
        let key = *key.as_bytes();
        let mut iv = [0u8; 16];
        iv.copy_from_slice(&key[..16]);

        Self {
            encryptor: Encryptor::new(&key.into(), &iv.into()),
            decryptor: Decryptor::new(&key.into(), &iv.into()),
            key,
            sent: 0,
            received: 0,
        }
    }

    fn checksum(&self, counter: u64, plaintext: &[u8]) -> [u8; CHECKSUM_LEN] {
        let mut input = Vec::with_capacity(8 + plaintext.len() + self.key.len());
        input.extend_from_slice(&counter.to_le_bytes());
        input.extend_from_slice(plaintext);
        input.extend_from_slice(&self.key);

        let mut out = [0u8; CHECKSUM_LEN];
        out.copy_from_slice(&digest::digest(&digest::SHA256, &input).as_ref()[..CHECKSUM_LEN]);
        out
    }

    /// Encrypts one packet, appending its checksum.
    pub fn encrypt(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let checksum = self.checksum(self.sent, plaintext);
        self.sent = self.sent.wrapping_add(1);

        let mut buf = Vec::with_capacity(plaintext.len() + CHECKSUM_LEN);
        buf.extend_from_slice(plaintext);
        buf.extend_from_slice(&checksum);

        // CFB8 has a one-byte block, so this walks the buffer while keeping the shift
        // register across packets — which is what makes it one stream and not many.
        for byte in &mut buf {
            let mut block = Block::<Encryptor>::from([*byte]);
            self.encryptor.encrypt_block_mut(&mut block);
            *byte = block[0];
        }
        buf
    }

    /// Decrypts one packet and checks its checksum.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, CipherError> {
        if ciphertext.len() < CHECKSUM_LEN {
            return Err(CipherError::TooShort);
        }

        let mut buf = ciphertext.to_vec();
        for byte in &mut buf {
            let mut block = Block::<Decryptor>::from([*byte]);
            self.decryptor.decrypt_block_mut(&mut block);
            *byte = block[0];
        }

        let split = buf.len() - CHECKSUM_LEN;
        let expected = self.checksum(self.received, &buf[..split]);

        // Constant time: a checksum compared byte by byte tells an attacker how much of
        // a forgery was right, one byte at a time.
        if !bool::from(buf[split..].ct_eq(&expected)) {
            return Err(CipherError::BadChecksum);
        }

        self.received = self.received.wrapping_add(1);
        buf.truncate(split);
        Ok(buf)
    }

    /// Packets encrypted so far.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Packets decrypted so far.
    pub fn received(&self) -> u64 {
        self.received
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::ServerKey;

    fn pair() -> (Cipher, Cipher) {
        let server = ServerKey::generate();
        let client = ServerKey::generate();
        let salt = *server.salt();
        let key = server.agree(client.public_key_der(), &salt).unwrap();
        let same = client.agree(server.public_key_der(), &salt).unwrap();
        assert_eq!(key, same);
        (Cipher::new(&key), Cipher::new(&same))
    }

    #[test]
    fn a_packet_round_trips() {
        let (mut a, mut b) = pair();
        let plaintext = b"\xff\x01\x04".to_vec();
        let encrypted = a.encrypt(&plaintext);
        assert_eq!(b.decrypt(&encrypted).unwrap(), plaintext);
    }

    /// The arithmetic that identified the mode: three bytes in, eleven out.
    #[test]
    fn the_length_matches_what_the_client_sent() {
        let (mut a, _) = pair();
        let encrypted = a.encrypt(b"\xff\x01\x04");
        assert_eq!(encrypted.len(), 3 + CHECKSUM_LEN);
        assert_eq!(
            encrypted.len(),
            11,
            "the client's payload was 0xfe plus eleven"
        );
    }

    /// One stream, not one per packet: the shift register carries over, so the same
    /// plaintext encrypts differently the second time.
    #[test]
    fn the_stream_carries_across_packets() {
        let (mut a, mut b) = pair();
        let first = a.encrypt(b"same");
        let second = a.encrypt(b"same");
        assert_ne!(first, second);

        assert_eq!(b.decrypt(&first).unwrap(), b"same");
        assert_eq!(b.decrypt(&second).unwrap(), b"same");
    }

    #[test]
    fn many_packets_stay_in_step() {
        let (mut a, mut b) = pair();
        for n in 0..64u8 {
            let plaintext = vec![n; usize::from(n) + 1];
            let encrypted = a.encrypt(&plaintext);
            assert_eq!(b.decrypt(&encrypted).unwrap(), plaintext, "packet {n}");
        }
        assert_eq!(a.sent(), 64);
        assert_eq!(b.received(), 64);
    }

    #[test]
    fn an_empty_packet_still_carries_a_checksum() {
        let (mut a, mut b) = pair();
        let encrypted = a.encrypt(b"");
        assert_eq!(encrypted.len(), CHECKSUM_LEN);
        assert!(b.decrypt(&encrypted).unwrap().is_empty());
    }

    #[test]
    fn a_tampered_packet_is_refused() {
        let (mut a, mut b) = pair();
        let mut encrypted = a.encrypt(b"payload");
        encrypted[0] ^= 0x01;
        assert_eq!(b.decrypt(&encrypted), Err(CipherError::BadChecksum));
    }

    /// A packet delivered twice must not verify: the counter has already moved.
    #[test]
    fn a_replayed_packet_is_refused() {
        let (mut a, mut b) = pair();
        let encrypted = a.encrypt(b"once");
        assert!(b.decrypt(&encrypted).is_ok());
        assert_eq!(b.decrypt(&encrypted), Err(CipherError::BadChecksum));
    }

    #[test]
    fn a_packet_shorter_than_its_checksum_is_refused() {
        let (_, mut b) = pair();
        assert_eq!(b.decrypt(&[0u8; 7]), Err(CipherError::TooShort));
        assert_eq!(b.decrypt(&[]), Err(CipherError::TooShort));
    }

    /// The wrong key fails at the checksum rather than yielding plausible rubbish.
    #[test]
    fn a_different_key_does_not_decrypt() {
        let (mut a, _) = pair();
        let (_, mut other) = pair();
        let encrypted = a.encrypt(b"secret");
        assert_eq!(other.decrypt(&encrypted), Err(CipherError::BadChecksum));
    }

    #[test]
    fn the_counter_does_not_appear_on_the_wire() {
        let (mut a, mut b) = pair();
        for _ in 0..300 {
            let encrypted = a.encrypt(b"x");
            assert_eq!(encrypted.len(), 1 + CHECKSUM_LEN, "no counter is sent");
            b.decrypt(&encrypted).unwrap();
        }
    }
}
