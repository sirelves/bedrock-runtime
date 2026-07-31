//! Verifying identity tokens whose signatures are real.
//!
//! The unit tests next to [`bedrock_crypto::jwt`] cover what can be refused before a
//! signature is checked: a forged `alg`, an unknown `kid`, a token that is not three
//! segments. Everything past the signature — expiry, issuer, audience, and the happy
//! path — needs a token that actually verifies, which needs a key we hold.
//!
//! So these tests mint their own. `fixtures/test-issuer.pkcs1.der` is a throwaway RSA key
//! generated for this file and nothing else; `fixtures/test-issuer.jwks.json` is its
//! public half, in the shape the real issuer publishes. **The private key is in the
//! repository on purpose** — it signs nothing anyone trusts, and the alternative is
//! tests that cannot cover the half of verification that matters.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bedrock_crypto::jwt::{self, Expected, Jwks, JwtError};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};

const KEY: &[u8] = include_bytes!("fixtures/test-issuer.pkcs1.der");
const JWKS: &str = include_str!("fixtures/test-issuer.jwks.json");

const ISSUER: &str = "https://authorization.franchise.minecraft-services.net/";
const AUDIENCE: &str = "api://auth-minecraft-services/multiplayer";

/// Midday of some day. Time is passed in, never read, so tests are not a clock away
/// from failing.
const NOW: i64 = 1_700_000_000;

fn keys() -> Jwks {
    Jwks::parse(JWKS).expect("the fixture is a key set")
}

fn expected() -> Expected {
    Expected {
        issuer: ISSUER.to_owned(),
        audience: AUDIENCE.to_owned(),
        leeway: 60,
    }
}

/// Signs a token with the test key.
fn mint(header: &str, claims: &str) -> String {
    let key = RsaKeyPair::from_der(KEY).expect("the fixture is a PKCS#1 RSA key");
    let signing_input = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(header),
        URL_SAFE_NO_PAD.encode(claims)
    );

    let mut signature = vec![0; key.public().modulus_len()];
    key.sign(
        &RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        signing_input.as_bytes(),
        &mut signature,
    )
    .expect("signing a token");

    format!("{signing_input}.{}", URL_SAFE_NO_PAD.encode(&signature))
}

/// A token with the claims a real one carries, overridable field by field.
fn token(iss: &str, aud: &str, iat: i64, exp: i64) -> String {
    mint(
        r#"{"alg":"RS256","kid":"test-issuer","typ":"JWT"}"#,
        &format!(
            r#"{{"iss":"{iss}","aud":"{aud}","iat":{iat},"exp":{exp},"xname":"TestPlayer","xid":"2535","cpk":"AAAA"}}"#
        ),
    )
}

fn valid() -> String {
    token(ISSUER, AUDIENCE, NOW - 60, NOW + 3600)
}

#[test]
fn a_token_the_issuer_signed_verifies() {
    let claims = jwt::verify(&valid(), &keys(), &expected(), NOW).expect("a valid token");

    assert_eq!(claims.iss, ISSUER);
    assert_eq!(claims.aud, AUDIENCE);
    assert_eq!(claims.xname.as_deref(), Some("TestPlayer"));
    assert_eq!(
        claims.cpk.as_deref(),
        Some("AAAA"),
        "the client key comes from the signed claims, not from the login body"
    );
}

/// The criterion of M0.3b, at this layer: a token that was genuine an hour ago is not
/// genuine now.
#[test]
fn an_expired_token_is_refused() {
    let expiry = NOW - 3600;
    let error = jwt::verify(
        &token(ISSUER, AUDIENCE, NOW - 7200, expiry),
        &keys(),
        &expected(),
        NOW,
    )
    .expect_err("an expired token");

    assert_eq!(error, JwtError::Expired { expiry, now: NOW });
}

/// The leeway is slack for clock skew, not a grace period: it must let a token that
/// just expired through and stop one that expired a minute later.
#[test]
fn expiry_is_judged_with_the_leeway_and_no_more() {
    let expiry = NOW - 30;
    let token = token(ISSUER, AUDIENCE, NOW - 3600, expiry);
    assert!(
        jwt::verify(&token, &keys(), &expected(), NOW).is_ok(),
        "30 seconds past expiry is inside a 60 second leeway"
    );

    let later = NOW + 31;
    assert_eq!(
        jwt::verify(&token, &keys(), &expected(), later).unwrap_err(),
        JwtError::Expired { expiry, now: later }
    );
}

#[test]
fn a_token_from_the_future_is_refused() {
    let issued = NOW + 3600;
    let error = jwt::verify(
        &token(ISSUER, AUDIENCE, issued, issued + 3600),
        &keys(),
        &expected(),
        NOW,
    )
    .expect_err("a token issued in the future");

    assert_eq!(error, JwtError::NotYetValid { issued, now: NOW });
}

/// A token minted for another service is a real token being replayed at ours.
#[test]
fn a_token_for_another_audience_is_refused() {
    let other = "api://someone-elses-service";
    let error = jwt::verify(
        &token(ISSUER, other, NOW - 60, NOW + 3600),
        &keys(),
        &expected(),
        NOW,
    )
    .expect_err("a token for another audience");

    assert_eq!(error, JwtError::WrongAudience(other.to_owned()));
}

#[test]
fn a_token_from_another_issuer_is_refused() {
    let other = "https://an-issuer-we-do-not-trust.test/";
    let error = jwt::verify(
        &token(other, AUDIENCE, NOW - 60, NOW + 3600),
        &keys(),
        &expected(),
        NOW,
    )
    .expect_err("a token from another issuer");

    assert_eq!(error, JwtError::WrongIssuer(other.to_owned()));
}

/// Editing the claims of a signed token is the attack the signature exists to stop:
/// take a real token, put someone else's name in it, keep the signature.
#[test]
fn claims_cannot_be_edited_after_signing() {
    let token = valid();
    let (signing_input, signature) = token.rsplit_once('.').expect("three segments");
    let (header, claims) = signing_input.split_once('.').expect("three segments");

    let mut edited: serde_json::Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(claims)
            .expect("claims are base64url"),
    )
    .expect("claims are JSON");
    edited["xname"] = serde_json::Value::String("SomeoneElse".to_owned());

    let forged = format!(
        "{header}.{}.{signature}",
        URL_SAFE_NO_PAD.encode(edited.to_string())
    );

    assert_eq!(
        jwt::verify(&forged, &keys(), &expected(), NOW).unwrap_err(),
        JwtError::BadSignature
    );
}

/// Key rotation: the issuer publishes several keys at once and the token says which one
/// signed it. A server that verified against "the first key" would refuse every valid
/// login the day the issuer rotates.
#[test]
fn the_key_id_selects_among_the_published_keys() {
    let published = keys();
    let ours = published.keys.first().expect("one key in the fixture");

    let rotated = format!(
        r#"{{"keys":[{{"kid":"an-older-key","kty":"RSA","n":"{}","e":"AQAB"}},{{"kid":"{}","kty":"RSA","n":"{}","e":"{}"}}]}}"#,
        "AQAB", ours.kid, ours.n, ours.e
    );
    let rotated = Jwks::parse(&rotated).expect("a key set with two keys");

    jwt::verify(&valid(), &rotated, &expected(), NOW)
        .expect("the token names its key, and that key is published");
}

/// The issuer's key set is fetched over HTTP, and a server that has not fetched it yet
/// cannot verify anything. It must refuse rather than wave logins through — the
/// property the whole milestone rests on.
#[test]
fn an_empty_key_set_verifies_nothing() {
    let empty = Jwks::parse(r#"{"keys":[]}"#).expect("an empty key set is still a key set");
    assert_eq!(
        jwt::verify(&valid(), &empty, &expected(), NOW).unwrap_err(),
        JwtError::UnknownKey("test-issuer".to_owned())
    );
}
