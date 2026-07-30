//! Runs the listener on a real UDP socket.
//!
//! ```text
//! cargo run -p bedrock-raknet --example serve -- [port]
//! ```
//!
//! Open Minecraft Bedrock on the same network; the server should appear under Friends.
//! It answers the RakNet handshake and nothing above it, so a client will connect and
//! then sit waiting for a login that never comes — which is exactly as far as M0.2 goes.
//!
//! This is the socket driver the sans-io design leaves to the caller (ADR-012): read,
//! feed, tick, drain.

use bedrock_raknet::DEFAULT_PORT_V4;
use bedrock_raknet::listener::{Event, Listener, ListenerConfig};
use std::error::Error;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

fn main() -> Result<(), Box<dyn Error>> {
    let port: u16 = match std::env::args().nth(1) {
        Some(arg) => arg.parse()?,
        None => DEFAULT_PORT_V4,
    };

    let socket = UdpSocket::bind(("0.0.0.0", port))?;
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let local = socket.local_addr()?;

    // The protocol and version numbers are Minecraft's, and this crate does not know
    // Minecraft — the real server passes them down from bedrock-protocol. Overridable
    // so this example can be pointed at whatever version is current.
    let protocol = std::env::var("BEDROCK_PROTOCOL").unwrap_or_else(|_| "1001".to_owned());
    let version = std::env::var("BEDROCK_VERSION").unwrap_or_else(|_| "1.26.30".to_owned());
    let guid: i64 = 0x0bed_0c00_0000_0003;

    let advertisement =
        format!("MCPE;bedrock-runtime;{protocol};{version};0;10;{guid};;Survival;1;{port};{port};");

    let mut listener = Listener::new(local, guid, &advertisement, ListenerConfig::default());

    println!("listening on {local}");
    println!("advertising  {advertisement}");
    println!("ctrl-c to stop\n");

    let dump = std::env::var("BEDROCK_DUMP").ok();
    let mut seen = 0usize;
    let mut buf = [0u8; 2048];
    loop {
        let now = Instant::now();

        if let Ok((len, from)) = socket.recv_from(&mut buf) {
            for event in listener.receive(from, &buf[..len], now) {
                match event {
                    Event::Connected(peer) => println!("connected     {peer}"),
                    Event::Disconnected(peer) => println!("disconnected  {peer}"),
                    Event::Payload(peer, payload) => {
                        println!("payload       {peer}  {} bytes", payload.len());
                        println!("  hex   {}", hex(&payload));
                        println!("  ascii {}", ascii(&payload));
                        let name = format!("payload-{:04}.bin", seen);
                        seen += 1;
                        if let Some(dir) = &dump {
                            let path = std::path::Path::new(dir).join(&name);
                            std::fs::create_dir_all(dir)?;
                            std::fs::write(&path, &payload)?;
                            println!("  saved {}", path.display());
                        }
                    }
                }
            }
        }

        for event in listener.tick(now) {
            if let Event::Disconnected(peer) = event {
                println!("timed out     {peer}");
            }
        }

        while let Some((to, datagram)) = listener.poll_transmit() {
            socket.send_to(&datagram, to)?;
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn ascii(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|&b| if b.is_ascii_graphic() { b as char } else { '.' })
        .collect()
}
