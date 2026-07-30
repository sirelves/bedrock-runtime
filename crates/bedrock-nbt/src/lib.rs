//! NBT in the variants Bedrock uses: little-endian (on disk) and network
//! little-endian (varint-encoded, on the wire).
//!
//! Leaf crate: no internal dependencies, no I/O, fuzzable in isolation.
//!
//! **Security:** nesting depth is bounded and every length prefix is checked
//! against a limit before allocating. See `SECURITY.md`.
//!
//! Status: not started.
