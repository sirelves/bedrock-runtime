//! Finding which variant of the key schedule a client actually uses.
//!
//! Three things are guessed at once — how the session key is derived, where the IV
//! comes from, and how the checksum is built — and all three fail the same way, at the
//! checksum. Guessing them one at a time costs a connection per attempt.
//!
//! This searches the whole space against a single captured packet instead. Eight bytes
//! of checksum agreeing is not chance, so the first combination that verifies is the
//! answer.
//!
//! Nothing here reveals key material: the search runs inside the crate that already
//! holds it, and returns only which combination worked.

use crate::agreement::{AgreementError, ServerKey};
use aes::Aes256;
use aes::cipher::{Block, BlockDecryptMut, KeyIvInit};
use ring::digest;
use std::fmt;

type Decryptor = cfb8::Decryptor<Aes256>;

/// How the session key is built from the salt and the shared secret.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Derivation {
    /// `SHA-256(salt || secret)`.
    SaltFirst,
    /// `SHA-256(secret || salt)`.
    SecretFirst,
}

/// Where the cipher's initial vector comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Iv {
    /// The first sixteen bytes of the session key.
    KeyPrefix,
    /// The salt itself, which is also sixteen bytes.
    Salt,
}

/// The order the checksum's input is assembled in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Checksum {
    /// `SHA-256(counter || plaintext || key)`.
    CounterPlaintextKey,
    /// `SHA-256(counter || key || plaintext)`.
    CounterKeyPlaintext,
    /// `SHA-256(key || counter || plaintext)`.
    KeyCounterPlaintext,
    /// `SHA-256(plaintext || key)`, with no counter at all.
    PlaintextKey,
}

/// One combination of the three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Variant {
    /// How the key is derived.
    pub derivation: Derivation,
    /// Where the IV comes from.
    pub iv: Iv,
    /// How the checksum is built.
    pub checksum: Checksum,
    /// What the counter starts at.
    pub counter_start: u64,
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "derivation={:?} iv={:?} checksum={:?} counter_start={}",
            self.derivation, self.iv, self.checksum, self.counter_start
        )
    }
}

/// What a matching variant produced, so the caller can sanity-check it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The combination that verified.
    pub variant: Variant,
    /// The plaintext it produced, checksum stripped.
    pub plaintext: Vec<u8>,
}

const DERIVATIONS: [Derivation; 2] = [Derivation::SaltFirst, Derivation::SecretFirst];
const IVS: [Iv; 2] = [Iv::KeyPrefix, Iv::Salt];
const CHECKSUMS: [Checksum; 4] = [
    Checksum::CounterPlaintextKey,
    Checksum::CounterKeyPlaintext,
    Checksum::KeyCounterPlaintext,
    Checksum::PlaintextKey,
];
const COUNTER_STARTS: [u64; 2] = [0, 1];

fn derive(kind: Derivation, salt: &[u8], secret: &[u8]) -> [u8; 32] {
    let mut input = Vec::with_capacity(salt.len() + secret.len());
    match kind {
        Derivation::SaltFirst => {
            input.extend_from_slice(salt);
            input.extend_from_slice(secret);
        }
        Derivation::SecretFirst => {
            input.extend_from_slice(secret);
            input.extend_from_slice(salt);
        }
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(digest::digest(&digest::SHA256, &input).as_ref());
    key
}

fn checksum(kind: Checksum, counter: u64, plaintext: &[u8], key: &[u8; 32]) -> [u8; 8] {
    let counter = counter.to_le_bytes();
    let mut input = Vec::with_capacity(8 + plaintext.len() + key.len());
    match kind {
        Checksum::CounterPlaintextKey => {
            input.extend_from_slice(&counter);
            input.extend_from_slice(plaintext);
            input.extend_from_slice(key);
        }
        Checksum::CounterKeyPlaintext => {
            input.extend_from_slice(&counter);
            input.extend_from_slice(key);
            input.extend_from_slice(plaintext);
        }
        Checksum::KeyCounterPlaintext => {
            input.extend_from_slice(key);
            input.extend_from_slice(&counter);
            input.extend_from_slice(plaintext);
        }
        Checksum::PlaintextKey => {
            input.extend_from_slice(plaintext);
            input.extend_from_slice(key);
        }
    }
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest::digest(&digest::SHA256, &input).as_ref()[..8]);
    out
}

/// Tries every combination against one captured packet.
///
/// `ciphertext` is the payload with the batch marker already stripped.
pub fn search(
    key: &ServerKey,
    peer_der: &[u8],
    salt: &[u8],
    ciphertext: &[u8],
) -> Result<Option<Found>, AgreementError> {
    if ciphertext.len() < 8 {
        return Ok(None);
    }
    let secret = key.shared_secret(peer_der)?;

    for derivation in DERIVATIONS {
        let session = derive(derivation, salt, &secret);

        for iv_kind in IVS {
            let mut iv = [0u8; 16];
            match iv_kind {
                Iv::KeyPrefix => iv.copy_from_slice(&session[..16]),
                Iv::Salt if salt.len() >= 16 => iv.copy_from_slice(&salt[..16]),
                Iv::Salt => continue,
            }

            let mut decryptor = Decryptor::new(&session.into(), &iv.into());
            let mut buf = ciphertext.to_vec();
            for byte in &mut buf {
                let mut block = Block::<Decryptor>::from([*byte]);
                decryptor.decrypt_block_mut(&mut block);
                *byte = block[0];
            }

            let split = buf.len() - 8;
            for checksum_kind in CHECKSUMS {
                for counter_start in COUNTER_STARTS {
                    let expected = checksum(checksum_kind, counter_start, &buf[..split], &session);
                    if buf[split..] == expected {
                        return Ok(Some(Found {
                            variant: Variant {
                                derivation,
                                iv: iv_kind,
                                checksum: checksum_kind,
                                counter_start,
                            },
                            plaintext: buf[..split].to_vec(),
                        }));
                    }
                }
            }
        }
    }
    Ok(None)
}

/// How many combinations [`search`] covers.
pub fn space() -> usize {
    DERIVATIONS.len() * IVS.len() * CHECKSUMS.len() * COUNTER_STARTS.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cipher::Cipher;

    /// The search must find the combination the shipped cipher uses, or it is testing
    /// something other than what we send.
    #[test]
    fn it_finds_the_variant_our_own_cipher_uses() {
        let server = ServerKey::generate();
        let client = ServerKey::generate();
        let salt = *server.salt();

        let session = client.agree(server.public_key_der(), &salt).unwrap();
        let mut cipher = Cipher::new(&session);
        let plaintext = b"\xff\x01\x04";
        let ciphertext = cipher.encrypt(plaintext);

        let found = search(&server, client.public_key_der(), &salt, &ciphertext)
            .unwrap()
            .expect("our own cipher must be in the space");

        assert_eq!(found.plaintext, plaintext);
        assert_eq!(found.variant.derivation, Derivation::SaltFirst);
        assert_eq!(found.variant.iv, Iv::KeyPrefix);
        assert_eq!(found.variant.checksum, Checksum::CounterPlaintextKey);
        assert_eq!(found.variant.counter_start, 0);
    }

    #[test]
    fn it_reports_nothing_for_bytes_that_are_not_ours() {
        let server = ServerKey::generate();
        let client = ServerKey::generate();
        let salt = *server.salt();

        let found = search(&server, client.public_key_der(), &salt, &[0u8; 11]).unwrap();
        assert!(found.is_none());
    }

    #[test]
    fn a_packet_too_short_to_hold_a_checksum_is_skipped() {
        let server = ServerKey::generate();
        let client = ServerKey::generate();
        let salt = *server.salt();
        assert!(
            search(&server, client.public_key_der(), &salt, &[0u8; 4])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn the_space_is_small_enough_to_search_on_a_live_connection() {
        assert_eq!(space(), 32);
    }
}
