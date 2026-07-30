//! Verifying the identity token a client presents at login.
//!
//! The token is an RS256 JWT issued by Minecraft's own authorization service, whose
//! signing keys are published as a JWKS. Verification here is pure: keys and the
//! current time come in as arguments, so it is testable against a captured token
//! without a clock or a network.
//!
//! Two things are deliberately not taken from the token: the algorithm and the key.
//! Reading `alg` from a token is the classic JWT forgery — a peer that picks `none`
//! or swaps RSA for HMAC verifies against itself. Here the algorithm is fixed and the
//! `kid` only *selects* among keys we already trust.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use serde::Deserialize;
use std::fmt;

/// A public key from the issuer's JWKS.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwk {
    /// Which key this is, matched against the token header.
    pub kid: String,
    /// Key type. Only `RSA` is accepted.
    pub kty: String,
    /// Modulus, base64url.
    pub n: String,
    /// Exponent, base64url.
    pub e: String,
}

/// The issuer's published keys.
#[derive(Debug, Clone, Deserialize)]
pub struct Jwks {
    /// The keys themselves.
    pub keys: Vec<Jwk>,
}

impl Jwks {
    /// Parses a JWKS document.
    pub fn parse(json: &str) -> Result<Self, JwtError> {
        serde_json::from_str(json).map_err(|_| JwtError::MalformedKeys)
    }

    fn find(&self, kid: &str) -> Option<&Jwk> {
        self.keys.iter().find(|key| key.kid == kid)
    }
}

/// What the token must say about itself before its claims are believed.
#[derive(Debug, Clone)]
pub struct Expected {
    /// Required `iss`.
    pub issuer: String,
    /// Required `aud`.
    pub audience: String,
    /// Slack for clock skew between us and the issuer, in seconds.
    pub leeway: i64,
}

/// Why a token was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwtError {
    /// Not three dot-separated segments.
    Malformed,
    /// A segment was not valid base64url, or its JSON did not parse.
    MalformedSegment(&'static str),
    /// The JWKS document did not parse.
    MalformedKeys,
    /// The header named an algorithm we do not accept.
    UnexpectedAlgorithm(String),
    /// The header named a key we do not have.
    UnknownKey(String),
    /// The key is not RSA.
    UnsupportedKeyType(String),
    /// The signature did not verify.
    BadSignature,
    /// `iss` was not what we require.
    WrongIssuer(String),
    /// `aud` was not what we require.
    WrongAudience(String),
    /// The token has expired.
    Expired {
        /// When it expired.
        expiry: i64,
        /// What we think the time is.
        now: i64,
    },
    /// The token is not valid yet.
    NotYetValid {
        /// When it becomes valid.
        issued: i64,
        /// What we think the time is.
        now: i64,
    },
}

impl fmt::Display for JwtError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => write!(f, "token is not three segments"),
            Self::MalformedSegment(which) => write!(f, "{which} segment did not decode"),
            Self::MalformedKeys => write!(f, "key set did not parse"),
            Self::UnexpectedAlgorithm(alg) => write!(f, "algorithm {alg} is not accepted"),
            Self::UnknownKey(kid) => write!(f, "no published key with id {kid}"),
            Self::UnsupportedKeyType(kty) => write!(f, "key type {kty} is not supported"),
            Self::BadSignature => write!(f, "signature did not verify"),
            Self::WrongIssuer(iss) => write!(f, "issued by {iss}"),
            Self::WrongAudience(aud) => write!(f, "audience is {aud}"),
            Self::Expired { expiry, now } => write!(f, "expired at {expiry}, now {now}"),
            Self::NotYetValid { issued, now } => write!(f, "issued at {issued}, now {now}"),
        }
    }
}

impl std::error::Error for JwtError {}

#[derive(Debug, Deserialize)]
struct Header {
    alg: String,
    kid: String,
}

/// The claims we act on. Everything else in the token is ignored on purpose.
#[derive(Debug, Clone, Deserialize)]
pub struct Claims {
    /// Issuer.
    pub iss: String,
    /// Audience.
    pub aud: String,
    /// Expiry, seconds since the epoch.
    pub exp: i64,
    /// Issued at, seconds since the epoch.
    pub iat: i64,
    /// The player's Xbox user id.
    pub xid: Option<String>,
    /// The player's gamertag.
    pub xname: Option<String>,
    /// The client's public key, base64 DER. This is what key agreement uses, and the
    /// only reason verifying the token matters for encryption: it binds the key to an
    /// identity the issuer vouched for.
    pub cpk: Option<String>,
}

fn decode_segment<T: serde::de::DeserializeOwned>(
    segment: &str,
    which: &'static str,
) -> Result<T, JwtError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(segment)
        .map_err(|_| JwtError::MalformedSegment(which))?;
    serde_json::from_slice(&bytes).map_err(|_| JwtError::MalformedSegment(which))
}

/// Verifies a token and returns its claims.
///
/// `now` is seconds since the epoch, passed in rather than read so this stays pure.
pub fn verify(token: &str, keys: &Jwks, expected: &Expected, now: i64) -> Result<Claims, JwtError> {
    let mut parts = token.split('.');
    let (Some(header_b64), Some(claims_b64), Some(signature_b64), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(JwtError::Malformed);
    };

    let header: Header = decode_segment(header_b64, "header")?;
    if header.alg != "RS256" {
        return Err(JwtError::UnexpectedAlgorithm(header.alg));
    }

    let key = keys
        .find(&header.kid)
        .ok_or_else(|| JwtError::UnknownKey(header.kid.clone()))?;
    if key.kty != "RSA" {
        return Err(JwtError::UnsupportedKeyType(key.kty.clone()));
    }

    let n = URL_SAFE_NO_PAD
        .decode(&key.n)
        .map_err(|_| JwtError::MalformedKeys)?;
    let e = URL_SAFE_NO_PAD
        .decode(&key.e)
        .map_err(|_| JwtError::MalformedKeys)?;
    let signature = URL_SAFE_NO_PAD
        .decode(signature_b64)
        .map_err(|_| JwtError::MalformedSegment("signature"))?;

    let signed = format!("{header_b64}.{claims_b64}");
    RsaPublicKeyComponents { n: &n, e: &e }
        .verify(&RSA_PKCS1_2048_8192_SHA256, signed.as_bytes(), &signature)
        .map_err(|_| JwtError::BadSignature)?;

    // Only after the signature holds are the claims worth reading.
    let claims: Claims = decode_segment(claims_b64, "claims")?;

    if claims.iss != expected.issuer {
        return Err(JwtError::WrongIssuer(claims.iss));
    }
    if claims.aud != expected.audience {
        return Err(JwtError::WrongAudience(claims.aud));
    }
    if now > claims.exp + expected.leeway {
        return Err(JwtError::Expired {
            expiry: claims.exp,
            now,
        });
    }
    if now + expected.leeway < claims.iat {
        return Err(JwtError::NotYetValid {
            issued: claims.iat,
            now,
        });
    }

    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    const KEYS: &str = r#"{"keys":[{"kid":"a","kty":"RSA","n":"AQAB","e":"AQAB","use":"sig"}]}"#;

    fn expected() -> Expected {
        Expected {
            issuer: "https://example.test/".to_owned(),
            audience: "api://test".to_owned(),
            leeway: 60,
        }
    }

    fn token(header: &str, claims: &str) -> String {
        format!(
            "{}.{}.{}",
            URL_SAFE_NO_PAD.encode(header),
            URL_SAFE_NO_PAD.encode(claims),
            URL_SAFE_NO_PAD.encode(b"not-a-real-signature")
        )
    }

    #[test]
    fn a_key_set_parses() {
        let keys = Jwks::parse(KEYS).unwrap();
        assert_eq!(keys.keys.len(), 1);
        assert!(keys.find("a").is_some());
        assert!(keys.find("missing").is_none());
    }

    /// The forgery this guards against: a token that names its own algorithm. Refused
    /// before any key is looked up.
    #[test]
    fn the_algorithm_comes_from_us_not_the_token() {
        for alg in ["none", "HS256", "RS512"] {
            let token = token(&format!(r#"{{"alg":"{alg}","kid":"a"}}"#), "{}");
            assert_eq!(
                verify(&token, &Jwks::parse(KEYS).unwrap(), &expected(), 0).unwrap_err(),
                JwtError::UnexpectedAlgorithm(alg.to_owned())
            );
        }
    }

    #[test]
    fn an_unknown_key_id_is_refused() {
        let token = token(r#"{"alg":"RS256","kid":"other"}"#, "{}");
        assert_eq!(
            verify(&token, &Jwks::parse(KEYS).unwrap(), &expected(), 0).unwrap_err(),
            JwtError::UnknownKey("other".to_owned())
        );
    }

    /// Claims are only read once the signature holds, so a bad signature never lets a
    /// malformed claim set matter.
    #[test]
    fn a_bad_signature_is_refused_before_claims_are_read() {
        let token = token(r#"{"alg":"RS256","kid":"a"}"#, "this is not json");
        assert_eq!(
            verify(&token, &Jwks::parse(KEYS).unwrap(), &expected(), 0).unwrap_err(),
            JwtError::BadSignature
        );
    }

    #[test]
    fn a_token_that_is_not_three_segments_is_refused() {
        for bad in ["", "one", "one.two", "one.two.three.four"] {
            assert_eq!(
                verify(bad, &Jwks::parse(KEYS).unwrap(), &expected(), 0).unwrap_err(),
                JwtError::Malformed,
                "{bad}"
            );
        }
    }

    #[test]
    fn a_malformed_key_set_is_refused() {
        assert_eq!(
            Jwks::parse("not json").unwrap_err(),
            JwtError::MalformedKeys
        );
        assert_eq!(Jwks::parse("{}").unwrap_err(), JwtError::MalformedKeys);
    }
}
