//! Key agreement: turning the client's public key into a session key.
//!
//! The server generates an ephemeral P-384 key pair and a random salt, agrees with the
//! client's key, and derives `SHA-256(salt || shared secret)`.
//!
//! **The derivation is a hypothesis.** It is what every implementation consulted does,
//! but nothing has confirmed it yet — the confirmation is a client accepting our
//! encrypted stream, which needs the cipher too. Until then, a login that fails after
//! this point is as likely to be this formula as anything downstream.
//!
//! Keys are never printed. [`SessionKey`] redacts itself in `Debug`, because a key that
//! reaches a log has left the process.

use ring::agreement::{self, ECDH_P384, EphemeralPrivateKey, UnparsedPublicKey};
use ring::digest;
use ring::rand::SecureRandom;
use std::fmt;

/// Bytes of a P-384 public key in SubjectPublicKeyInfo DER.
pub const PUBLIC_KEY_DER_LEN: usize = 120;

/// Bytes of an uncompressed P-384 point: the `0x04` tag and two 48-byte coordinates.
pub const POINT_LEN: usize = 97;

/// Bytes of salt mixed into the derivation.
pub const SALT_LEN: usize = 16;

/// The DER that precedes the point in a secp384r1 SubjectPublicKeyInfo.
///
/// Matched as a constant rather than parsed. One curve is supported, its encoding is
/// fixed, and a DER parser here would be a lot of attacker-facing surface to accept
/// exactly one byte string. Anything else is rejected.
const SPKI_PREFIX: [u8; 23] = [
    0x30, 0x76, 0x30, 0x10, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x05, 0x2b,
    0x81, 0x04, 0x00, 0x22, 0x03, 0x62, 0x00,
];

/// Why key agreement failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgreementError {
    /// The key was not a secp384r1 SubjectPublicKeyInfo.
    NotP384PublicKey,
    /// The point was not the uncompressed form.
    NotUncompressed,
    /// Generating a key or salt failed.
    NoRandomness,
    /// The peer's key was rejected by the curve — off-curve, or the point at infinity.
    BadPeerKey,
}

impl fmt::Display for AgreementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotP384PublicKey => write!(f, "not a secp384r1 public key"),
            Self::NotUncompressed => write!(f, "public key point is not uncompressed"),
            Self::NoRandomness => write!(f, "could not get randomness"),
            Self::BadPeerKey => write!(f, "peer public key is not a valid curve point"),
        }
    }
}

impl std::error::Error for AgreementError {}

/// Extracts the raw point from a SubjectPublicKeyInfo.
pub fn point_from_der(der: &[u8]) -> Result<&[u8], AgreementError> {
    if der.len() != PUBLIC_KEY_DER_LEN || !der.starts_with(&SPKI_PREFIX) {
        return Err(AgreementError::NotP384PublicKey);
    }
    let point = &der[SPKI_PREFIX.len()..];
    if point.first() != Some(&0x04) {
        return Err(AgreementError::NotUncompressed);
    }
    Ok(point)
}

/// Wraps a raw point in a SubjectPublicKeyInfo, the form the handshake sends.
pub fn point_to_der(point: &[u8]) -> Result<Vec<u8>, AgreementError> {
    if point.len() != POINT_LEN || point.first() != Some(&0x04) {
        return Err(AgreementError::NotUncompressed);
    }
    let mut der = Vec::with_capacity(PUBLIC_KEY_DER_LEN);
    der.extend_from_slice(&SPKI_PREFIX);
    der.extend_from_slice(point);
    Ok(der)
}

/// The symmetric key both sides end up with.
#[derive(Clone, PartialEq, Eq)]
pub struct SessionKey([u8; 32]);

impl SessionKey {
    /// The key bytes. Every caller of this is a place to check for leaks.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionKey(redacted)")
    }
}

/// One side's ephemeral key material, consumed by [`Self::agree`].
pub struct Handshake {
    private: EphemeralPrivateKey,
    public_der: Vec<u8>,
    salt: [u8; SALT_LEN],
}

impl fmt::Debug for Handshake {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Handshake(redacted)")
    }
}

impl Handshake {
    /// Generates an ephemeral key pair and a salt.
    pub fn new(rng: &dyn SecureRandom) -> Result<Self, AgreementError> {
        let private = EphemeralPrivateKey::generate(&ECDH_P384, rng)
            .map_err(|_| AgreementError::NoRandomness)?;
        let public = private
            .compute_public_key()
            .map_err(|_| AgreementError::NoRandomness)?;
        let public_der = point_to_der(public.as_ref())?;

        let mut salt = [0u8; SALT_LEN];
        rng.fill(&mut salt)
            .map_err(|_| AgreementError::NoRandomness)?;

        Ok(Self {
            private,
            public_der,
            salt,
        })
    }

    /// Our public key, as the DER the peer expects.
    pub fn public_key_der(&self) -> &[u8] {
        &self.public_der
    }

    /// The salt to send alongside it. The peer needs it to derive the same key.
    pub fn salt(&self) -> &[u8; SALT_LEN] {
        &self.salt
    }

    /// Agrees with a peer's public key and derives the session key.
    ///
    /// Takes the salt rather than using our own so the same code works on both sides:
    /// the side that receives a handshake derives with the salt it was sent.
    pub fn agree(self, peer_der: &[u8], salt: &[u8]) -> Result<SessionKey, AgreementError> {
        let peer_point = point_from_der(peer_der)?;
        let peer = UnparsedPublicKey::new(&ECDH_P384, peer_point);

        agreement::agree_ephemeral(self.private, &peer, |secret| {
            let mut input = Vec::with_capacity(salt.len() + secret.len());
            input.extend_from_slice(salt);
            input.extend_from_slice(secret);

            let mut key = [0u8; 32];
            key.copy_from_slice(digest::digest(&digest::SHA256, &input).as_ref());
            SessionKey(key)
        })
        .map_err(|_| AgreementError::BadPeerKey)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::rand::SystemRandom;

    /// The property that matters: two sides that exchanged public keys and a salt end
    /// up with the same key, and neither needed the other's private key.
    #[test]
    fn both_sides_derive_the_same_key() {
        let rng = SystemRandom::new();
        let server = Handshake::new(&rng).unwrap();
        let client = Handshake::new(&rng).unwrap();

        let salt = *server.salt();
        let server_public = server.public_key_der().to_vec();
        let client_public = client.public_key_der().to_vec();

        let a = server.agree(&client_public, &salt).unwrap();
        let b = client.agree(&server_public, &salt).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_salt_gives_a_different_key() {
        let rng = SystemRandom::new();
        let server = Handshake::new(&rng).unwrap();
        let client = Handshake::new(&rng).unwrap();
        let server_public = server.public_key_der().to_vec();
        let client_public = client.public_key_der().to_vec();

        let a = server.agree(&client_public, b"salt one--------").unwrap();
        let b = client.agree(&server_public, b"salt two--------").unwrap();
        assert_ne!(a, b);
    }

    #[test]
    fn a_generated_key_is_the_shape_the_protocol_expects() {
        let handshake = Handshake::new(&SystemRandom::new()).unwrap();
        assert_eq!(handshake.public_key_der().len(), PUBLIC_KEY_DER_LEN);
        assert_eq!(handshake.salt().len(), SALT_LEN);
        assert!(handshake.public_key_der().starts_with(&SPKI_PREFIX));
    }

    #[test]
    fn der_round_trips_through_the_point() {
        let handshake = Handshake::new(&SystemRandom::new()).unwrap();
        let der = handshake.public_key_der().to_vec();
        let point = point_from_der(&der).unwrap();
        assert_eq!(point.len(), POINT_LEN);
        assert_eq!(point_to_der(point).unwrap(), der);
    }

    /// The exact prefix a real client's key carried, from a captured login.
    #[test]
    fn the_captured_key_prefix_is_accepted() {
        let mut der = SPKI_PREFIX.to_vec();
        der.push(0x04);
        der.extend(std::iter::repeat_n(0x11, POINT_LEN - 1));
        assert_eq!(point_from_der(&der).unwrap().len(), POINT_LEN);
    }

    #[test]
    fn a_key_of_another_curve_is_refused() {
        let mut der = SPKI_PREFIX.to_vec();
        der[18] = 0x03; // a different curve OID
        der.push(0x04);
        der.extend(std::iter::repeat_n(0x11, POINT_LEN - 1));
        assert_eq!(
            point_from_der(&der).unwrap_err(),
            AgreementError::NotP384PublicKey
        );
    }

    #[test]
    fn a_compressed_point_is_refused() {
        let mut der = SPKI_PREFIX.to_vec();
        der.push(0x02); // compressed form
        der.extend(std::iter::repeat_n(0x11, POINT_LEN - 1));
        assert_eq!(
            point_from_der(&der).unwrap_err(),
            AgreementError::NotUncompressed
        );
    }

    #[test]
    fn a_key_of_the_wrong_length_is_refused() {
        for len in [0, 23, 119, 121, 200] {
            let der = vec![0u8; len];
            assert!(point_from_der(&der).is_err(), "{len}");
        }
    }

    /// A point that is the right shape but not on the curve must be rejected by the
    /// curve, not accepted into a derivation.
    #[test]
    fn a_point_off_the_curve_is_refused() {
        let rng = SystemRandom::new();
        let handshake = Handshake::new(&rng).unwrap();

        let mut der = SPKI_PREFIX.to_vec();
        der.push(0x04);
        der.extend(std::iter::repeat_n(0xaa, POINT_LEN - 1));

        assert_eq!(
            handshake.agree(&der, b"salt------------").unwrap_err(),
            AgreementError::BadPeerKey
        );
    }

    /// A key in a log has left the process.
    #[test]
    fn a_session_key_does_not_print_itself() {
        let rng = SystemRandom::new();
        let server = Handshake::new(&rng).unwrap();
        let client = Handshake::new(&rng).unwrap();
        let salt = *server.salt();
        let public = client.public_key_der().to_vec();

        let key = server.agree(&public, &salt).unwrap();
        let printed = format!("{key:?}");
        assert_eq!(printed, "SessionKey(redacted)");
        assert!(!printed.contains(&format!("{:02x}", key.as_bytes()[0])));
    }
}
