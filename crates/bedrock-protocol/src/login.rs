//! The `Login` packet: who the client claims to be.
//!
//! Mapped from a real client on protocol 1001, because the shape published by older
//! implementations is a previous version of it:
//!
//! ```text
//! int32 BE   client protocol version
//! varint     length of everything below
//!   int32 LE length + JSON  {"AuthenticationType":0,"Token":"<RS256 JWT>"}
//!   int32 LE length + JWT   ES384, public key in its x5u header
//! ```
//!
//! The two length prefixes inside the blob are **little-endian**, unlike the protocol
//! version above them and unlike the varints around them. Three conventions in one
//! packet.
//!
//! Nothing here is trusted. Signatures are not checked at this layer and the strings
//! are a peer's claims about itself; this module turns bytes into borrowed slices and
//! stops there.
//!
//! The captured login was 588 KB, 96% of it skin, so [`Limits`] exists and the decoder
//! borrows rather than copies.

use crate::bytes::{DecodeError, Reader};
use std::fmt;

/// `Login`, client to server.
pub const ID_LOGIN: u32 = 1;

/// Who must have issued a client's identity token, read from a real login.
pub const TOKEN_ISSUER: &str = "https://authorization.franchise.minecraft-services.net/";

/// The audience that token must be for. A token minted for a different audience is a
/// valid token being replayed at the wrong service.
pub const TOKEN_AUDIENCE: &str = "api://auth-minecraft-services/multiplayer";

/// Where the issuer publishes its signing keys, from its OIDC discovery document.
pub const TOKEN_KEYS_URL: &str =
    "https://authorization.franchise.minecraft-services.net/.well-known/keys";

/// What a login may cost before it is refused.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Longest identity document accepted.
    pub max_identity: usize,
    /// Longest client-data token accepted. Skins dominate this.
    pub max_client_data: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            // The observed identity blob was ~1 KB; the client data was 574 KB, almost
            // all of it skin. These leave room for a bigger skin without leaving room
            // for a peer to bill us for megabytes.
            max_identity: 64 * 1024,
            max_client_data: 2 * 1024 * 1024,
        }
    }
}

/// Why a login did not decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginError {
    /// The buffer ended early or a varint was malformed.
    Malformed(DecodeError),
    /// A section declared a length past what [`Limits`] allows.
    TooLarge {
        /// Which section.
        section: &'static str,
        /// What it declared.
        declared: usize,
        /// What is allowed.
        limit: usize,
    },
    /// A section was not valid UTF-8.
    NotUtf8(&'static str),
}

impl From<DecodeError> for LoginError {
    fn from(e: DecodeError) -> Self {
        Self::Malformed(e)
    }
}

impl fmt::Display for LoginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(e) => write!(f, "{e}"),
            Self::TooLarge {
                section,
                declared,
                limit,
            } => write!(f, "{section} declared {declared} bytes, limit is {limit}"),
            Self::NotUtf8(section) => write!(f, "{section} is not valid UTF-8"),
        }
    }
}

impl std::error::Error for LoginError {}

/// A decoded login, borrowing from the packet body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Login<'a> {
    /// The protocol version the client speaks. Compare before anything else: a
    /// mismatch here explains every later failure.
    pub client_protocol: u32,
    /// JSON holding the authentication token. Unverified.
    pub identity: &'a str,
    /// A JWT of client data — skin, device, language. Unverified.
    pub client_data: &'a str,
}

impl<'a> Login<'a> {
    /// Decodes a login body.
    pub fn decode(body: &'a [u8], limits: &Limits) -> Result<Self, LoginError> {
        let mut r = Reader::new(body);
        let client_protocol = r.u32_be()?;

        // The blob length is redundant with the section lengths inside it. Reading
        // through it rather than trusting it means a lie costs a short read.
        let blob = r.prefixed()?;
        let mut r = Reader::new(blob);

        let identity = section(&mut r, "identity", limits.max_identity)?;
        let client_data = section(&mut r, "client data", limits.max_client_data)?;

        Ok(Self {
            client_protocol,
            identity,
            client_data,
        })
    }
}

fn section<'a>(
    r: &mut Reader<'a>,
    name: &'static str,
    limit: usize,
) -> Result<&'a str, LoginError> {
    let declared = r.u32()? as usize;
    if declared > limit {
        return Err(LoginError::TooLarge {
            section: name,
            declared,
            limit,
        });
    }
    std::str::from_utf8(r.bytes(declared)?).map_err(|_| LoginError::NotUtf8(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bytes::Writer;

    /// Builds a login with the real framing and invented contents.
    ///
    /// Synthetic on purpose: a real login carries the player's XUID, gamertag, device
    /// id and skin, and this repository is public.
    fn login(protocol: u32, identity: &str, client_data: &str) -> Vec<u8> {
        let mut blob = Writer::new();
        blob.u32(u32::try_from(identity.len()).unwrap_or(u32::MAX))
            .bytes(identity.as_bytes())
            .u32(u32::try_from(client_data.len()).unwrap_or(u32::MAX))
            .bytes(client_data.as_bytes());

        let mut w = Writer::new();
        w.u32_be(protocol).prefixed(&blob.finish());
        w.finish()
    }

    #[test]
    fn a_login_round_trips() {
        let identity = r#"{"AuthenticationType":0,"Token":"eyJhbGciOiJSUzI1NiJ9.e30.sig"}"#;
        let client_data = "eyJhbGciOiJFUzM4NCJ9.e30.sig";
        let body = login(1001, identity, client_data);

        let decoded = Login::decode(&body, &Limits::default()).unwrap();
        assert_eq!(decoded.client_protocol, 1001);
        assert_eq!(decoded.identity, identity);
        assert_eq!(decoded.client_data, client_data);
    }

    /// The framing that came off the wire, byte for byte, with the tokens replaced.
    #[test]
    fn the_captured_framing_is_reproduced() {
        let body = login(1001, "{}", "x");
        assert_eq!(
            &body[..4],
            &[0x00, 0x00, 0x03, 0xe9],
            "protocol is big-endian"
        );

        // varint blob length, then the identity length little-endian.
        let mut r = Reader::new(&body[4..]);
        let blob = r.prefixed().unwrap();
        assert_eq!(
            &blob[..4],
            &[0x02, 0x00, 0x00, 0x00],
            "section length is little-endian"
        );
    }

    /// The version is the first field for a reason: reading it costs nothing and it
    /// explains every later failure.
    #[test]
    fn the_protocol_version_is_readable_before_anything_else() {
        let body = login(999, "{}", "x");
        assert_eq!(
            Login::decode(&body, &Limits::default())
                .unwrap()
                .client_protocol,
            999
        );
    }

    #[test]
    fn a_section_beyond_the_limit_is_refused_before_reading_it() {
        let limits = Limits {
            max_identity: 8,
            ..Limits::default()
        };
        let body = login(1001, "an identity longer than eight bytes", "x");
        assert!(matches!(
            Login::decode(&body, &limits),
            Err(LoginError::TooLarge { .. })
        ));
    }

    /// A length that overshoots the buffer must fail as a short read, not allocate.
    #[test]
    fn a_lying_section_length_fails_cleanly() {
        let mut blob = Writer::new();
        blob.u32(9_999).bytes(b"short");
        let mut w = Writer::new();
        w.u32_be(1001).prefixed(&blob.finish());

        assert!(matches!(
            Login::decode(&w.finish(), &Limits::default()),
            Err(LoginError::Malformed(_))
        ));
    }

    #[test]
    fn invalid_utf8_is_named_not_guessed() {
        let mut blob = Writer::new();
        blob.u32(2).bytes(&[0xff, 0xfe]).u32(0);
        let mut w = Writer::new();
        w.u32_be(1001).prefixed(&blob.finish());

        assert_eq!(
            Login::decode(&w.finish(), &Limits::default()),
            Err(LoginError::NotUtf8("identity"))
        );
    }

    #[test]
    fn truncated_logins_never_panic() {
        let full = login(1001, r#"{"AuthenticationType":0}"#, "token");
        for n in 0..full.len() {
            let _ = Login::decode(&full[..n], &Limits::default());
        }
    }

    /// Borrowed, not copied: a login is hundreds of kilobytes and almost all of it is
    /// skin data nobody at this layer looks at.
    #[test]
    fn decoding_borrows_from_the_body() {
        let client_data = "e".repeat(100_000);
        let body = login(1001, "{}", &client_data);
        let decoded = Login::decode(&body, &Limits::default()).unwrap();
        assert!(std::ptr::eq(
            decoded.client_data.as_ptr(),
            body[body.len() - client_data.len()..].as_ptr()
        ));
    }
}
