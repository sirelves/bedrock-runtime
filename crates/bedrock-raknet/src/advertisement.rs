//! The semicolon-separated string a Bedrock server puts in its pong.
//!
//! This layout is a Bedrock convention layered on top of RakNet, not part of RakNet
//! itself. It was confirmed against four live servers on 2026-07-30; the captures are
//! in `tests/fixtures/` and the test that pins them is `tests/advertisement.rs`.
//!
//! ```text
//! MCPE;<motd>;<protocol>;<version>;<online>;<max>;<guid>;<sub-motd>;<gamemode>[;...]
//!  0     1        2          3        4      5     6         7          8
//! ```
//!
//! Two things the captures proved, both of which shape this API:
//!
//! **The field count varies.** The four servers sent 9, 9, 10 and 13 fields. Some end
//! with a trailing `;`, which yields an empty final field. So nothing here is
//! mandatory: every accessor returns `Option`, and a short advertisement is not an
//! error.
//!
//! **The values are a claim, not a fact.** Two of the four servers advertised a
//! protocol version that cannot be real — one said `121`, another said `1` — while
//! reporting player counts like `20001/100001`. Large networks front a multi-version
//! proxy and put filler in these fields. Treat anything in here as what the operator
//! chose to say, and never as authority about the protocol. See
//! `docs/COMPATIBILITY.md` for how the target version is actually decided.

/// A parsed advertisement string.
///
/// Parsing cannot fail: an advertisement is whatever the server chose to send, and
/// rejecting it would only hide data we want to look at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advertisement {
    fields: Vec<String>,
}

impl Advertisement {
    /// Splits an advertisement into its fields.
    pub fn parse(s: &str) -> Self {
        Self {
            fields: s.split(';').map(str::to_owned).collect(),
        }
    }

    /// The field at `index`, if the server sent that many.
    pub fn field(&self, index: usize) -> Option<&str> {
        self.fields.get(index).map(String::as_str)
    }

    /// How many fields the server sent. Observed range: 9 to 13.
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// Whether there are no fields at all.
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }

    /// Every field, in order.
    pub fn fields(&self) -> impl Iterator<Item = &str> {
        self.fields.iter().map(String::as_str)
    }

    /// Field 0 — the edition tag, `MCPE` on every server observed.
    pub fn edition(&self) -> Option<&str> {
        self.field(0)
    }

    /// Field 1 — the first MOTD line.
    pub fn motd(&self) -> Option<&str> {
        self.field(1)
    }

    /// Field 2 — the protocol version the server *claims* to speak.
    ///
    /// Unreliable by itself: see the module docs. Corroborate across several servers,
    /// or read it off a server you control.
    pub fn protocol_version(&self) -> Option<u32> {
        self.field(2)?.parse().ok()
    }

    /// Field 3 — the human-readable version string the server claims.
    pub fn version_name(&self) -> Option<&str> {
        self.field(3)
    }

    /// Field 4 — players online, as claimed.
    pub fn online_players(&self) -> Option<u64> {
        self.field(4)?.parse().ok()
    }

    /// Field 5 — player slots, as claimed.
    pub fn max_players(&self) -> Option<u64> {
        self.field(5)?.parse().ok()
    }

    /// Field 6 — the server GUID, repeated here from the pong header.
    pub fn server_guid(&self) -> Option<i64> {
        self.field(6)?.parse().ok()
    }

    /// Field 7 — the second MOTD line. Often empty.
    pub fn sub_motd(&self) -> Option<&str> {
        self.field(7)
    }

    /// Field 8 — the default gamemode, as a name.
    pub fn gamemode(&self) -> Option<&str> {
        self.field(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_documented_fields() {
        let a = Advertisement::parse("MCPE;hello;1001;1.26.30;7;16;123;sub;Survival");
        assert_eq!(a.edition(), Some("MCPE"));
        assert_eq!(a.motd(), Some("hello"));
        assert_eq!(a.protocol_version(), Some(1001));
        assert_eq!(a.version_name(), Some("1.26.30"));
        assert_eq!(a.online_players(), Some(7));
        assert_eq!(a.max_players(), Some(16));
        assert_eq!(a.server_guid(), Some(123));
        assert_eq!(a.sub_motd(), Some("sub"));
        assert_eq!(a.gamemode(), Some("Survival"));
    }

    #[test]
    fn missing_fields_are_none_not_errors() {
        let a = Advertisement::parse("MCPE;hello");
        assert_eq!(a.len(), 2);
        assert_eq!(a.motd(), Some("hello"));
        assert_eq!(a.protocol_version(), None);
        assert_eq!(a.gamemode(), None);
    }

    #[test]
    fn a_trailing_semicolon_yields_an_empty_field() {
        let a = Advertisement::parse("MCPE;hello;");
        assert_eq!(a.len(), 3);
        assert_eq!(a.field(2), Some(""));
    }

    #[test]
    fn a_non_numeric_number_is_none_not_a_panic() {
        let a = Advertisement::parse("MCPE;x;not-a-number;1.0;a;b;c;;Survival");
        assert_eq!(a.protocol_version(), None);
        assert_eq!(a.online_players(), None);
        assert_eq!(a.version_name(), Some("1.0"));
    }

    #[test]
    fn an_empty_string_still_parses() {
        let a = Advertisement::parse("");
        assert_eq!(a.len(), 1);
        assert_eq!(a.edition(), Some(""));
    }
}
