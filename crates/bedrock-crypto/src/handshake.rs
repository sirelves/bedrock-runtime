//! Building the token that hands the client our public key and salt.
//!
//! ```text
//! header   {"alg":"ES384","x5u":"<base64 DER of our public key>"}
//! payload  {"salt":"<base64 salt>"}
//! ```
//!
//! Self-signed on purpose: the client verifies it with the key inside `x5u`, which
//! proves we hold the matching private key, and then agrees with that same key.
//!
//! # Two things here are unconfirmed
//!
//! Which base64 alphabet `x5u` and `salt` use. The client's own key arrived as standard
//! base64 with padding, so that is what this emits, but the salt could be url-safe and
//! nothing has proved otherwise.
//!
//! Both fail the same way if wrong: the client derives a different session key, says
//! nothing, and goes quiet. That is why [`crate::agreement`] and this module are worth
//! testing together against a real client rather than separately against a document.

use crate::agreement::ServerKey;
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};

/// Builds the signed handshake token.
pub fn token(key: &ServerKey) -> String {
    let header = format!(
        r#"{{"alg":"ES384","x5u":"{}"}}"#,
        STANDARD.encode(key.public_key_der())
    );
    let payload = format!(r#"{{"salt":"{}"}}"#, STANDARD.encode(key.salt()));

    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(payload)
    );
    let signature = key.sign(signing_input.as_bytes());

    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(signature))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agreement::{PUBLIC_KEY_DER_LEN, SALT_LEN, point_from_der};
    use p384::PublicKey;
    use p384::ecdsa::signature::Verifier;
    use p384::ecdsa::{Signature, VerifyingKey};
    use serde_json::Value;

    fn segments(token: &str) -> (Value, Value, Vec<u8>) {
        let parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3, "a JWT is three segments");
        let decode = |s: &str| URL_SAFE_NO_PAD.decode(s).unwrap();
        (
            serde_json::from_slice(&decode(parts[0])).unwrap(),
            serde_json::from_slice(&decode(parts[1])).unwrap(),
            decode(parts[2]),
        )
    }

    #[test]
    fn the_header_names_es384_and_carries_our_key() {
        let key = ServerKey::generate();
        let (header, _, _) = segments(&token(&key));

        assert_eq!(header["alg"], "ES384");
        let der = STANDARD.decode(header["x5u"].as_str().unwrap()).unwrap();
        assert_eq!(der.len(), PUBLIC_KEY_DER_LEN);
        assert_eq!(der, key.public_key_der());
    }

    #[test]
    fn the_payload_carries_the_salt() {
        let key = ServerKey::generate();
        let (_, payload, _) = segments(&token(&key));

        let salt = STANDARD.decode(payload["salt"].as_str().unwrap()).unwrap();
        assert_eq!(salt.len(), SALT_LEN);
        assert_eq!(salt, key.salt());
    }

    /// What the client will do with it: verify the signature using the key the token
    /// itself supplies.
    #[test]
    fn the_token_verifies_under_the_key_it_carries() {
        let key = ServerKey::generate();
        let token = token(&key);
        let (header, _, signature) = segments(&token);

        let der = STANDARD.decode(header["x5u"].as_str().unwrap()).unwrap();
        let public = PublicKey::from_sec1_bytes(point_from_der(&der).unwrap()).unwrap();
        let verifying = VerifyingKey::from(&public);

        let signed_input = token.rsplit_once('.').unwrap().0;
        let parsed = Signature::from_slice(&signature).unwrap();
        assert!(verifying.verify(signed_input.as_bytes(), &parsed).is_ok());
    }

    /// A token from one key must not verify under another's, or the signature proves
    /// nothing about who sent it.
    #[test]
    fn a_token_does_not_verify_under_another_key() {
        let mine = ServerKey::generate();
        let other = ServerKey::generate();
        let token = token(&mine);
        let (_, _, signature) = segments(&token);

        let der = other.public_key_der();
        let public = PublicKey::from_sec1_bytes(point_from_der(der).unwrap()).unwrap();
        let verifying = VerifyingKey::from(&public);

        let signed_input = token.rsplit_once('.').unwrap().0;
        let parsed = Signature::from_slice(&signature).unwrap();
        assert!(verifying.verify(signed_input.as_bytes(), &parsed).is_err());
    }

    #[test]
    fn the_signature_is_the_fixed_jose_form() {
        let (_, _, signature) = segments(&token(&ServerKey::generate()));
        assert_eq!(signature.len(), 96);
    }

    #[test]
    fn two_sessions_produce_different_tokens() {
        assert_ne!(token(&ServerKey::generate()), token(&ServerKey::generate()));
    }
}
