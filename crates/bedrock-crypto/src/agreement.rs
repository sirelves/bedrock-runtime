//! The server's P-384 key: used to agree a session key, and to sign the handshake that
//! carries it.
//!
//! One key doing both is the protocol's requirement, not a shortcut. The client reads
//! the public key out of the handshake's `x5u` header, verifies the signature with it,
//! and then agrees with that same key. Signing with a different key than the one in
//! `x5u` would either fail verification or be a lie about who holds what.
//!
//! Derivation is `SHA-256(salt || shared secret)`. **Still a hypothesis** — every
//! implementation consulted does it this way and nothing has confirmed it. The
//! confirmation is a client accepting our encrypted stream, which needs the cipher too.
//!
//! Keys are never printed: [`SessionKey`] and [`ServerKey`] redact themselves, because
//! a key that reaches a log has left the process.

use p384::PublicKey;
use p384::ecdh::diffie_hellman;
use p384::ecdsa::signature::Signer;
use p384::ecdsa::{Signature, SigningKey};
use p384::elliptic_curve::rand_core::OsRng;
use p384::elliptic_curve::sec1::ToEncodedPoint;
use ring::digest;
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
/// exactly one byte string. Read off a real client's key.
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
    /// The peer's key was rejected by the curve — off-curve, or the point at infinity.
    BadPeerKey,
}

impl fmt::Display for AgreementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotP384PublicKey => write!(f, "not a secp384r1 public key"),
            Self::NotUncompressed => write!(f, "public key point is not uncompressed"),
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

/// The server's key pair for one session.
pub struct ServerKey {
    signing: SigningKey,
    public_der: Vec<u8>,
    salt: [u8; SALT_LEN],
}

impl fmt::Debug for ServerKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ServerKey(redacted)")
    }
}

impl Default for ServerKey {
    fn default() -> Self {
        Self::generate()
    }
}

impl ServerKey {
    /// Generates a key pair and a salt from the operating system's randomness.
    pub fn generate() -> Self {
        let signing = SigningKey::random(&mut OsRng);
        let public = PublicKey::from(signing.verifying_key());
        let point = public.to_encoded_point(false);

        let mut public_der = Vec::with_capacity(PUBLIC_KEY_DER_LEN);
        public_der.extend_from_slice(&SPKI_PREFIX);
        public_der.extend_from_slice(point.as_bytes());

        let mut salt = [0u8; SALT_LEN];
        getrandom(&mut salt);

        Self {
            signing,
            public_der,
            salt,
        }
    }

    /// Our public key, as the DER that goes in the handshake's `x5u`.
    pub fn public_key_der(&self) -> &[u8] {
        &self.public_der
    }

    /// The salt the peer needs to derive the same key.
    pub fn salt(&self) -> &[u8; SALT_LEN] {
        &self.salt
    }

    /// Signs `message` in the fixed `r || s` form JOSE uses.
    ///
    /// Not DER: a JWT signature is the two 48-byte scalars concatenated, and emitting
    /// ASN.1 here produces a token every client rejects.
    pub fn sign(&self, message: &[u8]) -> [u8; 96] {
        let signature: Signature = self.signing.sign(message);
        let bytes = signature.to_bytes();
        let mut out = [0u8; 96];
        out.copy_from_slice(&bytes);
        out
    }

    /// The raw ECDH output, for [`crate::probe`] to try derivations against.
    ///
    /// Crate-private: the shared secret is the one value that must not leave here.
    pub(crate) fn shared_secret(&self, peer_der: &[u8]) -> Result<Vec<u8>, AgreementError> {
        let point = point_from_der(peer_der)?;
        let peer = PublicKey::from_sec1_bytes(point).map_err(|_| AgreementError::BadPeerKey)?;
        let shared = diffie_hellman(self.signing.as_nonzero_scalar(), peer.as_affine());
        Ok(shared.raw_secret_bytes().to_vec())
    }

    /// Agrees with a peer's public key and derives the session key.
    ///
    /// Takes the salt rather than always using our own, so the same code works from
    /// either side of the exchange.
    pub fn agree(&self, peer_der: &[u8], salt: &[u8]) -> Result<SessionKey, AgreementError> {
        let point = point_from_der(peer_der)?;
        let peer = PublicKey::from_sec1_bytes(point).map_err(|_| AgreementError::BadPeerKey)?;

        let shared = diffie_hellman(self.signing.as_nonzero_scalar(), peer.as_affine());

        let mut input = Vec::with_capacity(salt.len() + 48);
        input.extend_from_slice(salt);
        input.extend_from_slice(shared.raw_secret_bytes());

        let mut key = [0u8; 32];
        key.copy_from_slice(digest::digest(&digest::SHA256, &input).as_ref());
        Ok(SessionKey(key))
    }
}

fn getrandom(out: &mut [u8]) {
    use ring::rand::SecureRandom;
    // Failing to get randomness is not a condition to paper over, and it does not
    // happen on a working system; the salt would otherwise be silently predictable.
    let rng = ring::rand::SystemRandom::new();
    if rng.fill(out).is_err() {
        std::process::abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use p384::ecdsa::VerifyingKey;
    use p384::ecdsa::signature::Verifier;

    /// The property that matters: two sides that exchanged public keys and a salt end
    /// up with the same key, and neither needed the other's private key.
    #[test]
    fn both_sides_derive_the_same_key() {
        let server = ServerKey::generate();
        let client = ServerKey::generate();
        let salt = *server.salt();

        let a = server.agree(client.public_key_der(), &salt).unwrap();
        let b = client.agree(server.public_key_der(), &salt).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn a_different_salt_gives_a_different_key() {
        let server = ServerKey::generate();
        let client = ServerKey::generate();

        let a = server
            .agree(client.public_key_der(), b"salt one--------")
            .unwrap();
        let b = client
            .agree(server.public_key_der(), b"salt two--------")
            .unwrap();
        assert_ne!(a, b);
    }

    /// The reason one key does both: the signature must verify under the very key the
    /// handshake hands over for agreement.
    #[test]
    fn the_signing_key_is_the_agreement_key() {
        let server = ServerKey::generate();
        let message = b"header.claims";
        let signature = server.sign(message);

        let point = point_from_der(server.public_key_der()).unwrap();
        let public = PublicKey::from_sec1_bytes(point).unwrap();
        let verifying = VerifyingKey::from(&public);

        let parsed = Signature::from_slice(&signature).unwrap();
        assert!(verifying.verify(message, &parsed).is_ok());
    }

    /// JOSE wants the raw scalars, not ASN.1. A DER signature is longer and rejected.
    #[test]
    fn signatures_are_the_fixed_jose_form() {
        let signature = ServerKey::generate().sign(b"anything");
        assert_eq!(signature.len(), 96, "two 48-byte scalars");
    }

    #[test]
    fn a_generated_key_is_the_shape_the_protocol_expects() {
        let server = ServerKey::generate();
        assert_eq!(server.public_key_der().len(), PUBLIC_KEY_DER_LEN);
        assert_eq!(server.salt().len(), SALT_LEN);
        assert!(server.public_key_der().starts_with(&SPKI_PREFIX));
    }

    #[test]
    fn two_sessions_do_not_share_a_key_or_a_salt() {
        let a = ServerKey::generate();
        let b = ServerKey::generate();
        assert_ne!(a.public_key_der(), b.public_key_der());
        assert_ne!(a.salt(), b.salt());
    }

    #[test]
    fn der_round_trips_through_the_point() {
        let server = ServerKey::generate();
        let der = server.public_key_der().to_vec();
        let point = point_from_der(&der).unwrap();
        assert_eq!(point.len(), POINT_LEN);
        assert_eq!(point_to_der(point).unwrap(), der);
    }

    #[test]
    fn a_key_of_another_curve_is_refused() {
        let mut der = SPKI_PREFIX.to_vec();
        der[18] = 0x03;
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
        der.push(0x02);
        der.extend(std::iter::repeat_n(0x11, POINT_LEN - 1));
        assert_eq!(
            point_from_der(&der).unwrap_err(),
            AgreementError::NotUncompressed
        );
    }

    #[test]
    fn a_key_of_the_wrong_length_is_refused() {
        for len in [0, 23, 119, 121, 200] {
            assert!(point_from_der(&vec![0u8; len]).is_err(), "{len}");
        }
    }

    /// A point with the right shape that is not on the curve must be rejected, not
    /// fed into a derivation.
    #[test]
    fn a_point_off_the_curve_is_refused() {
        let mut der = SPKI_PREFIX.to_vec();
        der.push(0x04);
        der.extend(std::iter::repeat_n(0xaa, POINT_LEN - 1));
        assert_eq!(
            ServerKey::generate()
                .agree(&der, b"salt------------")
                .unwrap_err(),
            AgreementError::BadPeerKey
        );
    }

    #[test]
    fn keys_do_not_print_themselves() {
        let server = ServerKey::generate();
        let client = ServerKey::generate();
        let salt = *server.salt();
        let key = server.agree(client.public_key_der(), &salt).unwrap();

        assert_eq!(format!("{server:?}"), "ServerKey(redacted)");
        let printed = format!("{key:?}");
        assert_eq!(printed, "SessionKey(redacted)");
        assert!(!printed.contains(&format!("{:02x}", key.as_bytes()[0])));
    }
}
