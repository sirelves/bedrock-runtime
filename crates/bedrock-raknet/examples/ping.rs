//! Sends an `UnconnectedPing` to a Bedrock server and prints what comes back.
//!
//! This is the M0.1a probe: the smallest thing that turns a claim about the protocol
//! into evidence. It is how `PROTOCOL_VERSION` in `bedrock-protocol` gets filled in —
//! see `docs/COMPATIBILITY.md`.
//!
//! ```text
//! cargo run -p bedrock-raknet --example ping -- <host>[:port] [fixture.bin]
//! ```
//!
//! The optional second argument writes the raw pong to disk. Captured bytes are test
//! artifacts, not trivia: `docs/PROTOCOL.md` requires that every confirmed behaviour
//! be pinned by a fixture.

use bedrock_raknet::offline::{decode_unconnected_pong, encode_unconnected_ping};
use bedrock_raknet::{DEFAULT_PORT_V4, MAGIC};
use std::error::Error;
use std::net::{ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next() else {
        eprintln!(
            "usage: ping <host>[:port] [fixture.bin]\n\
             \n\
             Sends a RakNet UnconnectedPing and prints the server's advertisement.\n\
             Port defaults to {DEFAULT_PORT_V4}."
        );
        std::process::exit(2);
    };
    let fixture = args.next();

    let target = if target.contains(':') {
        target
    } else {
        format!("{target}:{DEFAULT_PORT_V4}")
    };
    let addr = target
        .to_socket_addrs()?
        .next()
        .ok_or("host did not resolve to any address")?;

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;

    let now = i64::try_from(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis())?;
    let ping = encode_unconnected_ping(now, 0x0bed_0c00_0000_0001);

    println!("-> {addr}  UnconnectedPing ({} bytes)", ping.len());
    let sent_at = Instant::now();
    socket.send_to(&ping, addr)?;

    // 4 KiB is well past any advertisement seen in the wild and far under the UDP
    // datagram limit. If a pong ever needs more than this, that is itself a finding.
    let mut buf = [0u8; 4096];
    let (len, from) = socket.recv_from(&mut buf)?;
    let rtt = sent_at.elapsed();
    let raw = &buf[..len];

    println!("<- {from}  {len} bytes in {:.1?}\n", rtt);

    let pong = decode_unconnected_pong(raw)?;
    println!("time echoed  {}", pong.time);
    println!("server guid  {:#018x}", pong.server_guid);
    println!("advertisement\n  {}\n", pong.advertisement);

    println!("fields, in order — the layout is unconfirmed, see docs/PROTOCOL.md:");
    for (i, field) in pong.advertisement.split(';').enumerate() {
        println!("  [{i:>2}] {field}");
    }

    if pong.time != now {
        println!("\nnote: the echoed time does not match the one we sent");
    }
    if !raw.windows(MAGIC.len()).any(|w| w == MAGIC) {
        println!("\nnote: magic not found in the raw datagram");
    }

    if let Some(path) = fixture {
        std::fs::write(&path, raw)?;
        println!("\nraw pong written to {path}");
    }

    Ok(())
}
