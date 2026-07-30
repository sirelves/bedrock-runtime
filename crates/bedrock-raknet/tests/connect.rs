//! Pins the opening handshake against replies captured from a live server on
//! 2026-07-30, via `cargo run -p bedrock-raknet --example connect`.
//!
//! `open-reply2-*.bin` is raw except for one documented edit: the four octets carrying
//! the client address were replaced with `203.0.113.5` (TEST-NET-3), because the
//! original is the capturing machine's public address and this repository is public.
//! Everything the test asserts on is untouched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bedrock_raknet::connect::{decode_reply_1, decode_reply_2, payload_limit};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

#[test]
fn reply_1_decodes() {
    let reply = decode_reply_1(&fixture("open-reply1-nethergames.bin")).unwrap();
    assert_eq!(reply.cookie, None, "this server runs without security");
    assert_eq!(reply.mtu, 1520);
    assert_ne!(reply.server_guid, 0);
}

#[test]
fn reply_2_decodes() {
    let reply = decode_reply_2(&fixture("open-reply2-nethergames.bin")).unwrap();
    assert_eq!(reply.mtu, 1500);
    assert!(!reply.encryption_enabled);
    assert_eq!(reply.client_addr.ip().to_string(), "203.0.113.5");
}

/// The server answered a 1492-byte probe with 1520, then settled on 1500 — the
/// Ethernet MTU, headers included. Taking 1500 as a payload size would put every
/// full datagram 28 bytes over.
#[test]
fn advertised_mtu_includes_the_headers() {
    let reply1 = decode_reply_1(&fixture("open-reply1-nethergames.bin")).unwrap();
    let reply2 = decode_reply_2(&fixture("open-reply2-nethergames.bin")).unwrap();

    assert_eq!(payload_limit(reply1.mtu), 1492, "what we probed with");
    assert_eq!(payload_limit(reply2.mtu), 1472);
}

#[test]
fn both_replies_carry_the_same_server_guid() {
    let reply1 = decode_reply_1(&fixture("open-reply1-nethergames.bin")).unwrap();
    let reply2 = decode_reply_2(&fixture("open-reply2-nethergames.bin")).unwrap();
    assert_eq!(reply1.server_guid, reply2.server_guid);
}
