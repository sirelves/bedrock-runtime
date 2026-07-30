//! Pins the offline decoder against pongs captured from live servers.
//!
//! Every byte in `fixtures/` came off the wire on 2026-07-30, from
//! `cargo run -p bedrock-raknet --example ping -- <host> <fixture>`. These are the
//! evidence `docs/PROTOCOL.md` demands before a claim about the protocol counts as
//! true — the unit tests next to the decoder use a hand-built pong, which proves
//! nothing about real servers.
//!
//! The MOTD text will drift as these servers change their branding, so nothing here
//! asserts on it. What is asserted is structure: the packet decodes, the field layout
//! holds, and the field *count* varies.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bedrock_raknet::advertisement::Advertisement;
use bedrock_raknet::offline::decode_unconnected_pong;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn advertisement_of(name: &str) -> Advertisement {
    let pong = decode_unconnected_pong(&fixture(name)).expect("captured pong must decode");
    Advertisement::parse(&pong.advertisement)
}

/// All four captures decode, and all four start with the `MCPE` tag.
#[test]
fn every_capture_decodes() {
    for name in [
        "pong-galaxite.bin",
        "pong-hive.bin",
        "pong-lbsg.bin",
        "pong-nethergames.bin",
    ] {
        let a = advertisement_of(name);
        assert_eq!(a.edition(), Some("MCPE"), "{name}");
        assert!(a.len() >= 9, "{name} had only {} fields", a.len());
    }
}

/// The reason no field is mandatory: real servers disagree on how many to send.
#[test]
fn field_count_varies_between_servers() {
    assert_eq!(advertisement_of("pong-galaxite.bin").len(), 9);
    assert_eq!(advertisement_of("pong-hive.bin").len(), 9);
    assert_eq!(advertisement_of("pong-nethergames.bin").len(), 10);
    assert_eq!(advertisement_of("pong-lbsg.bin").len(), 13);
}

/// Two independent servers agreeing is what makes 1001 a usable hypothesis.
#[test]
fn corroborating_servers_report_the_same_protocol() {
    let lbsg = advertisement_of("pong-lbsg.bin");
    let ng = advertisement_of("pong-nethergames.bin");

    assert_eq!(lbsg.protocol_version(), Some(1001));
    assert_eq!(ng.protocol_version(), Some(1001));
    assert_eq!(lbsg.version_name(), Some("1.26.30"));
    assert_eq!(ng.version_name(), Some("1.26.30"));
}

/// And this is why one server is never enough.
///
/// Hive claims protocol 121 with 20001 players online out of 100001; Galaxite claims
/// protocol 1. Both are live, popular servers. The numbers are filler in front of a
/// multi-version proxy, and a decoder that trusted them would be wrong.
#[test]
fn some_servers_advertise_filler() {
    let hive = advertisement_of("pong-hive.bin");
    assert_eq!(hive.protocol_version(), Some(121));
    assert_eq!(hive.online_players(), Some(20001));
    assert_eq!(hive.max_players(), Some(100001));

    assert_eq!(
        advertisement_of("pong-galaxite.bin").protocol_version(),
        Some(1)
    );
}

/// A trailing `;` is common, and shows up as an empty last field rather than as noise.
#[test]
fn trailing_separator_becomes_an_empty_field() {
    let ng = advertisement_of("pong-nethergames.bin");
    assert_eq!(ng.field(ng.len() - 1), Some(""));
}

/// The GUID appears twice — in the pong header and again inside the string. When a
/// server fills both honestly they agree, which is a cheap sanity check on the offset
/// arithmetic in the decoder.
#[test]
fn guid_in_header_matches_guid_in_string() {
    let raw = fixture("pong-lbsg.bin");
    let pong = decode_unconnected_pong(&raw).unwrap();
    let a = Advertisement::parse(&pong.advertisement);
    assert_eq!(a.server_guid(), Some(pong.server_guid));
}
