//! Pins the opening handshake against replies captured from a live server on
//! 2026-07-30, via `cargo run -p bedrock-raknet --example connect`.
//!
//! `open-reply2-*.bin` is raw except for one documented edit: the four octets carrying
//! the client address were replaced with `203.0.113.5` (TEST-NET-3), because the
//! original is the capturing machine's public address and this repository is public.
//! Everything the test asserts on is untouched.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use bedrock_raknet::address::is_empty_slot;
use bedrock_raknet::connect::{decode_reply_1, decode_reply_2, payload_limit};
use bedrock_raknet::online::{decode_connected_pong, decode_connection_request_accepted};

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

/// Captured from a completed connection. The client address is redacted the same way
/// as in reply 2.
#[test]
fn connection_request_accepted_decodes() {
    let accepted =
        decode_connection_request_accepted(&fixture("connection-request-accepted-nethergames.bin"))
            .unwrap();

    assert_eq!(accepted.system_index, 0);
    assert_eq!(accepted.client_addr.ip().to_string(), "203.0.113.5");
    assert_eq!(
        accepted.system_addresses.len(),
        20,
        "this server sends twenty"
    );
    assert!(
        accepted.system_addresses.iter().copied().all(is_empty_slot),
        "all twenty are empty, as 0.0.0.0:0 rather than RakNet's 255.255.255.255:0"
    );
}

#[test]
fn connected_pong_decodes() {
    let pong = decode_connected_pong(&fixture("connected-pong-nethergames.bin")).unwrap();
    assert!(pong.ping_time > 0);
    assert!(pong.pong_time > 0);
}
