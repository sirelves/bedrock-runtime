//! Server binary: argument parsing, the UDP socket, logging, shutdown.
//!
//! The layers below are sans-io (ADR-012), so this is where the loop lives: read a
//! datagram, feed it in, tick, drain what comes out.
//!
//! Game logic in this crate is a bug.

use bedrock_server::server::{DEFAULT_PORT, Event, Server, TARGET_PROTOCOL, advertisement};
use std::error::Error;
use std::net::UdpSocket;
use std::path::Path;
use std::time::{Duration, Instant};

const GUID: i64 = 0x0bed_0c00_0000_0003;

struct Options {
    port: u16,
    name: String,
    dump: Option<String>,
}

fn main() -> Result<(), Box<dyn Error>> {
    let Some(options) = parse_args()? else {
        return Ok(());
    };

    let socket = UdpSocket::bind(("0.0.0.0", options.port))?;
    socket.set_read_timeout(Some(Duration::from_millis(50)))?;
    let local = socket.local_addr()?;

    let advertisement = advertisement(&options.name, 0, 10, options.port, GUID);
    let mut server = Server::new(local, GUID, &advertisement);

    println!("bedrock-runtime {}", env!("CARGO_PKG_VERSION"));
    println!("listening on  {local}");
    println!("advertising   {advertisement}");
    if let Some(dir) = &options.dump {
        println!("capturing to  {dir}");
    }
    println!("ctrl-c to stop\n");

    let mut captured = 0usize;
    let mut buf = [0u8; 2048];

    loop {
        let now = Instant::now();
        let mut events = Vec::new();

        if let Ok((len, from)) = socket.recv_from(&mut buf) {
            events.extend(server.receive(from, &buf[..len], now));
        }
        events.extend(server.tick(now));

        for event in events {
            report(&event, &options, &mut captured)?;
        }

        while let Some((to, datagram)) = server.poll_transmit() {
            socket.send_to(&datagram, to)?;
        }
    }
}

fn report(event: &Event, options: &Options, captured: &mut usize) -> Result<(), Box<dyn Error>> {
    match event {
        Event::Connected(peer) => println!("connected     {peer}"),
        Event::Disconnected(peer) => println!("disconnected  {peer}"),
        Event::NetworkSettingsRequested {
            peer,
            client_protocol,
        } => {
            println!("handshake     {peer}  client speaks protocol {client_protocol}");
            if *client_protocol != TARGET_PROTOCOL {
                println!("  note: we target {TARGET_PROTOCOL}, so this client will stall at login");
            }
            println!("  -> NetworkSettings, compression off");
        }
        Event::Unhandled { peer, id, body } => {
            println!("packet {id:>4}   {peer}  {} bytes", body.len());
            println!("  head  {}", head(body));
            save(options, captured, &format!("packet-{id}"), body)?;
        }
        Event::Compressed { peer, method } => {
            println!("compressed    {peer}  batch declares {method:?}, which we cannot read yet");
        }
        Event::Undecodable(peer, payload) => {
            println!("undecodable   {peer}  {} bytes", payload.len());
            println!("  head  {}", head(payload));
            save(options, captured, "undecodable", payload)?;
        }
    }
    Ok(())
}

fn save(
    options: &Options,
    captured: &mut usize,
    label: &str,
    bytes: &[u8],
) -> Result<(), Box<dyn Error>> {
    let Some(dir) = &options.dump else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    let path = Path::new(dir).join(format!("{captured:04}-{label}.bin"));
    *captured += 1;
    std::fs::write(&path, bytes)?;
    println!("  saved {}", path.display());
    Ok(())
}

/// First bytes as hex, enough to recognise a packet without flooding the terminal.
fn head(bytes: &[u8]) -> String {
    let shown = bytes.len().min(48);
    let mut out = bytes
        .iter()
        .take(shown)
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ");
    if bytes.len() > shown {
        out.push_str(&format!("  ... +{} bytes", bytes.len() - shown));
    }
    out
}

fn parse_args() -> Result<Option<Options>, Box<dyn Error>> {
    let mut options = Options {
        port: DEFAULT_PORT,
        name: "bedrock-runtime".to_owned(),
        dump: None,
    };
    let mut args = std::env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => {
                print_help();
                return Ok(None);
            }
            "-V" | "--version" => {
                println!("bedrock-runtime {}", env!("CARGO_PKG_VERSION"));
                return Ok(None);
            }
            "--port" => options.port = args.next().ok_or("--port needs a value")?.parse()?,
            "--name" => options.name = args.next().ok_or("--name needs a value")?,
            "--dump" => options.dump = Some(args.next().ok_or("--dump needs a value")?),
            other => {
                eprintln!("unknown argument: {other}");
                print_help();
                std::process::exit(2);
            }
        }
    }
    Ok(Some(options))
}

fn print_help() {
    println!(
        "bedrock-runtime — a Minecraft: Bedrock Edition server in Rust

USAGE:
    bedrock-cli [OPTIONS]

OPTIONS:
    --port <PORT>    UDP port to bind (default {DEFAULT_PORT})
    --name <NAME>    Name shown in the client's server list
    --dump <DIR>     Write unhandled packets there, for protocol work
    -h, --help       Print this message
    -V, --version    Print the version

STATUS:
    The transport is complete and a real client connects. The login sequence is
    not implemented, so a client connects and then waits. See docs/ROADMAP.md."
    );
}
