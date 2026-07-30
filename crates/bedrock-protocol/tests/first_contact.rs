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
//! The client was on 26.23 (protocol 975) while the target is 1001. What this fixture
//! pins is the framing, which is what M0.3 has to decode either way — and the fact that
//! the client states its protocol version in the clear, before any encryption, which is
//! the cheapest possible confirmation of a target version.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

const BATCH_MARKER: u8 = 0xfe;
const REQUEST_NETWORK_SETTINGS: u32 = 193;

fn fixture() -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/request-network-settings-975.bin",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
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
    let raw = fixture();
    let mut at = 0;

    assert_eq!(raw[at], BATCH_MARKER, "batches start with 0xfe");
    at += 1;

    let len = varint(&raw, &mut at) as usize;
    assert_eq!(len, 6);

    let start = at;
    let id = varint(&raw, &mut at);
    assert_eq!(id, REQUEST_NETWORK_SETTINGS);

    let body = &raw[at..start + len];
    assert_eq!(body.len(), 4, "the body is one int32");
    assert_eq!(u32::from_be_bytes(body.try_into().unwrap()), 975);
}

/// The batch length covers the packet and nothing else — no trailing bytes, which is
/// what makes a batch of several packets decodable one after another.
#[test]
fn the_batch_length_accounts_for_every_byte() {
    let raw = fixture();
    let mut at = 1;
    let len = varint(&raw, &mut at) as usize;
    assert_eq!(at + len, raw.len());
}

/// Before any encryption is negotiated, the client says which protocol it speaks. A
/// mismatch is therefore knowable without implementing a single line of crypto.
#[test]
fn the_protocol_version_arrives_in_the_clear() {
    let raw = fixture();
    assert_eq!(u32::from_be_bytes(raw[4..8].try_into().unwrap()), 975);
}
