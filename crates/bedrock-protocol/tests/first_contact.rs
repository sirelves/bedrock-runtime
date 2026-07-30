//! The first game-layer bytes a real Bedrock client ever sent us.
//!
//! Captured on 2026-07-30 from an unmodified client that found the server, completed
//! the RakNet handshake and opened the login sequence. Eight bytes, and they carry the
//! shape of everything above RakNet:
//!
//! ```text
//! fe        batch marker
//! 06        varint: 6 bytes of packet
//! c1 01     varint 193 = RequestNetworkSettings
//! 00 00 03 cf   int32 big-endian = 975, the protocol the client speaks
//! ```
//!
//! Two captures, from the same client before and after updating it. The first read 975
//! (release 26.23), the second 1001 — which is the target, confirmed from a client
//! rather than inferred from servers.
//!
//! That the client states its protocol version in the clear, before any encryption, as
//! the very first thing it sends, makes confirming a target version cost one connection
//! and no cryptography at all.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

const BATCH_MARKER: u8 = 0xfe;
const REQUEST_NETWORK_SETTINGS: u32 = 193;

fn fixture(protocol: u32) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/request-network-settings-{protocol}.bin",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

/// Reads the protocol version a capture declares.
fn declared_protocol(raw: &[u8]) -> u32 {
    let mut at = 1;
    let len = varint(raw, &mut at) as usize;
    let start = at;
    assert_eq!(varint(raw, &mut at), REQUEST_NETWORK_SETTINGS);
    let body = &raw[at..start + len];
    u32::from_be_bytes(body.try_into().unwrap())
}

/// Reads a Bedrock varint: seven bits per byte, low group first.
fn varint(bytes: &[u8], at: &mut usize) -> u32 {
    let mut value = 0u32;
    let mut shift = 0;
    while let Some(&byte) = bytes.get(*at) {
        *at += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
        shift += 7;
    }
    value
}

#[test]
fn the_first_packet_is_a_batched_request_for_network_settings() {
    for protocol in [975, 1001] {
        let raw = fixture(protocol);
        let mut at = 0;

        assert_eq!(raw[at], BATCH_MARKER, "batches start with 0xfe");
        at += 1;

        let len = varint(&raw, &mut at) as usize;
        assert_eq!(len, 6);

        let start = at;
        assert_eq!(varint(&raw, &mut at), REQUEST_NETWORK_SETTINGS);

        let body = &raw[at..start + len];
        assert_eq!(body.len(), 4, "the body is one int32");
        assert_eq!(u32::from_be_bytes(body.try_into().unwrap()), protocol);
    }
}

/// The constant the server runs on and the number a real client sent must not drift
/// apart. If this fails, either the target moved or the capture is stale — and finding
/// out here costs nothing, while finding out in M0.3 costs a day of debugging crypto
/// that was never broken.
#[test]
fn the_target_matches_what_a_real_client_declared() {
    assert_eq!(
        declared_protocol(&fixture(1001)),
        bedrock_protocol::version::PROTOCOL_VERSION
    );
}

/// The batch length covers the packet and nothing else — no trailing bytes, which is
/// what makes a batch of several packets decodable one after another.
#[test]
fn the_batch_length_accounts_for_every_byte() {
    let raw = fixture(1001);
    let mut at = 1;
    let len = varint(&raw, &mut at) as usize;
    assert_eq!(at + len, raw.len());
}

/// Before any encryption is negotiated, the client says which protocol it speaks. A
/// mismatch is therefore knowable without implementing a single line of crypto.
#[test]
fn the_protocol_version_arrives_in_the_clear() {
    assert_eq!(declared_protocol(&fixture(1001)), 1001);
}
