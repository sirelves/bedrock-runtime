//! Walks the RakNet opening handshake against a real server and reports what came back.
//!
//! ```text
//! cargo run -p bedrock-raknet --example connect -- <host>[:port] [dir]
//! ```
//!
//! `dir` writes the raw replies. Note that reply 2 contains *your* public address, so
//! anything committed from it has to be redacted first.

use bedrock_raknet::connect::{
    ConnectError, ID_INCOMPATIBLE_PROTOCOL_VERSION, ID_OPEN_CONNECTION_REPLY_1,
    ID_OPEN_CONNECTION_REPLY_2, MTU_LADDER, PROTOCOL_VERSION, decode_incompatible, decode_reply_1,
    decode_reply_2, encode_request_1, encode_request_2,
};
use bedrock_raknet::wire::Reader;
use bedrock_raknet::{DEFAULT_PORT_V4, address};
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::time::Duration;

const CLIENT_GUID: i64 = 0x0bed_0c00_0000_0002;

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let Some(target) = args.next() else {
        eprintln!("usage: connect <host>[:port] [dir]");
        std::process::exit(2);
    };
    let out_dir = args.next();

    let target = if target.contains(':') {
        target
    } else {
        format!("{target}:{DEFAULT_PORT_V4}")
    };
    let addr = target
        .to_socket_addrs()?
        .next()
        .ok_or("host did not resolve")?;

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    println!("local {}  ->  {addr}\n", socket.local_addr()?);

    let mut buf = [0u8; 2048];

    // RAKNET_MTU pins a single rung, which is how the MTU semantics get probed.
    let ladder: Vec<usize> = match std::env::var("RAKNET_MTU") {
        Ok(v) => vec![v.parse()?],
        Err(_) => MTU_LADDER.to_vec(),
    };

    for mtu in ladder {
        let request = encode_request_1(mtu).ok_or("MTU below the minimum packet size")?;
        println!("-> OpenConnectionRequest1  mtu={mtu} protocol={PROTOCOL_VERSION}");
        socket.send_to(&request, addr)?;

        let len = match socket.recv_from(&mut buf) {
            Ok((len, _)) => len,
            Err(_) => {
                println!("   no reply, dropping to the next MTU\n");
                continue;
            }
        };
        let raw = &buf[..len];

        match raw.first() {
            Some(&ID_OPEN_CONNECTION_REPLY_1) => {
                let reply = decode_reply_1(raw)?;
                println!("<- OpenConnectionReply1  {len} bytes");
                println!("   guid    {:#018x}", reply.server_guid);
                println!("   mtu     {}", reply.mtu);
                println!(
                    "   cookie  {}\n",
                    match reply.cookie {
                        Some(c) => format!("{c:#010x}  (security on)"),
                        None => "none  (security off)".to_owned(),
                    }
                );
                save(&out_dir, "reply1.bin", raw)?;
                return finish(&socket, addr, &mut buf, reply.mtu, reply.cookie, &out_dir);
            }
            Some(&ID_INCOMPATIBLE_PROTOCOL_VERSION) => {
                let incompatible = decode_incompatible(raw)?;
                println!(
                    "<- IncompatibleProtocolVersion: server speaks {}, we sent {PROTOCOL_VERSION}",
                    incompatible.server_protocol
                );
                save(&out_dir, "incompatible.bin", raw)?;
                return Ok(());
            }
            other => {
                println!("<- unexpected first byte {other:?}, {len} bytes");
                return Ok(());
            }
        }
    }

    println!("no server answered at any MTU");
    Ok(())
}

fn finish(
    socket: &UdpSocket,
    addr: SocketAddr,
    buf: &mut [u8],
    mtu: u16,
    cookie: Option<u32>,
    out_dir: &Option<String>,
) -> Result<(), Box<dyn Error>> {
    println!("-> OpenConnectionRequest2  mtu={mtu}");
    socket.send_to(&encode_request_2(addr, mtu, CLIENT_GUID, cookie), addr)?;

    let (len, _) = socket.recv_from(buf)?;
    let raw = &buf[..len];

    if raw.first() != Some(&ID_OPEN_CONNECTION_REPLY_2) {
        println!("<- unexpected reply, first byte {:?}", raw.first());
        return Ok(());
    }

    let reply = decode_reply_2(raw)?;
    println!("<- OpenConnectionReply2  {len} bytes");
    println!("   guid        {:#018x}", reply.server_guid);
    println!("   mtu         {}", reply.mtu);
    println!("   encryption  {}", reply.encryption_enabled);

    // The complement quirk is still a hypothesis, so show both readings and let the
    // reader compare against an address they already know.
    let mut r = Reader::new(&raw[1 + 16 + 8..]);
    let raw_reading = address::read_raw(&mut r).map_err(ConnectError::from)?;
    println!("   our address, complemented  {}", reply.client_addr);
    println!("   our address, as sent       {raw_reading}");

    save(out_dir, "reply2.bin", raw)?;
    Ok(())
}

fn save(dir: &Option<String>, name: &str, bytes: &[u8]) -> std::io::Result<()> {
    let Some(dir) = dir else { return Ok(()) };
    std::fs::create_dir_all(dir)?;
    let path = Path::new(dir).join(name);
    std::fs::write(&path, bytes)?;
    println!("   saved {}", path.display());
    Ok(())
}
