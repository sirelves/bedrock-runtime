//! Opens a full RakNet connection against a real server and reports each step.
//!
//! ```text
//! cargo run -p bedrock-raknet --example connect -- <host>[:port] [dir]
//! RAKNET_MTU=1200 cargo run ...      pin one rung of the MTU ladder
//! ```
//!
//! `dir` writes the raw replies. Reply 2 and ConnectionRequestAccepted both contain
//! *your* public address, so anything committed from them has to be redacted.

use bedrock_raknet::connect::{
    ID_INCOMPATIBLE_PROTOCOL_VERSION, ID_OPEN_CONNECTION_REPLY_1, ID_OPEN_CONNECTION_REPLY_2,
    MTU_LADDER, PROTOCOL_VERSION, decode_incompatible, decode_reply_1, decode_reply_2,
    encode_request_1, encode_request_2, payload_limit,
};
use bedrock_raknet::datagram::{Acknowledgement, Datagram, FrameSet};
use bedrock_raknet::frame::{Frame, Reliability};
use bedrock_raknet::online::{
    ID_CONNECTED_PING, ID_CONNECTED_PONG, ID_CONNECTION_REQUEST_ACCEPTED, decode_connected_pong,
    decode_connection_request_accepted, encode_connected_ping, encode_connection_request,
    encode_disconnect, encode_new_incoming_connection,
};
use bedrock_raknet::wire::Writer;
use bedrock_raknet::{DEFAULT_PORT_V4, address};
use std::error::Error;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant};

const CLIENT_GUID: i64 = 0x0bed_0c00_0000_0002;

/// Agreed MTU and the cookie to echo, if the server asked for one.
struct Opened {
    mtu: u16,
    cookie: Option<u32>,
}

struct Session {
    socket: UdpSocket,
    server: SocketAddr,
    out_dir: Option<String>,
    started: Instant,
    next_sequence: u32,
    next_reliable: u32,
    next_order: u32,
}

impl Session {
    fn now(&self) -> i64 {
        i64::try_from(self.started.elapsed().as_millis()).unwrap_or(i64::MAX)
    }

    /// Wraps a payload in a reliable-ordered frame and sends it as its own datagram.
    fn send_reliable(&mut self, payload: Vec<u8>) -> std::io::Result<()> {
        let frame = Frame {
            reliability: Reliability::ReliableOrdered,
            reliable_index: self.next_reliable,
            sequence_index: 0,
            order_index: self.next_order,
            order_channel: 0,
            split: None,
            payload,
        };
        self.next_reliable += 1;
        self.next_order += 1;

        let datagram = Datagram::FrameSet(FrameSet {
            sequence: self.next_sequence,
            frames: vec![frame],
        });
        self.next_sequence += 1;

        let mut w = Writer::new();
        datagram.encode(&mut w);
        self.socket.send_to(&w.finish(), self.server)?;
        Ok(())
    }

    fn ack(&mut self, sequence: u32) -> std::io::Result<()> {
        let mut w = Writer::new();
        Datagram::Ack(Acknowledgement::single(sequence)).encode(&mut w);
        self.socket.send_to(&w.finish(), self.server)?;
        Ok(())
    }

    fn save(&self, name: &str, bytes: &[u8]) -> std::io::Result<()> {
        let Some(dir) = &self.out_dir else {
            return Ok(());
        };
        std::fs::create_dir_all(dir)?;
        let path = Path::new(dir).join(name);
        std::fs::write(&path, bytes)?;
        println!("   saved {}", path.display());
        Ok(())
    }
}

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
    let server = target
        .to_socket_addrs()?
        .next()
        .ok_or("host did not resolve")?;

    let socket = UdpSocket::bind("0.0.0.0:0")?;
    socket.set_read_timeout(Some(Duration::from_secs(5)))?;
    println!("local {}  ->  {server}\n", socket.local_addr()?);

    let mut session = Session {
        socket,
        server,
        out_dir,
        started: Instant::now(),
        next_sequence: 0,
        next_reliable: 0,
        next_order: 0,
    };

    let mut buf = [0u8; 2048];
    let Some(opened) = open(&mut session, &mut buf)? else {
        return Ok(());
    };

    println!("-> OpenConnectionRequest2  mtu={}", opened.mtu);
    let request = encode_request_2(server, opened.mtu, CLIENT_GUID, opened.cookie);
    session.socket.send_to(&request, server)?;

    let (len, _) = session.socket.recv_from(&mut buf)?;
    if buf.first() != Some(&ID_OPEN_CONNECTION_REPLY_2) {
        println!("<- unexpected reply to request 2");
        return Ok(());
    }
    let reply2 = decode_reply_2(&buf[..len])?;
    println!("<- OpenConnectionReply2  mtu={}", reply2.mtu);
    println!("   usable payload {} bytes\n", payload_limit(reply2.mtu));
    session.save("reply2.bin", &buf[..len])?;

    connected(&mut session, &mut buf)
}

/// Walks the MTU ladder until a server answers. Returns the agreed MTU and cookie.
fn open(session: &mut Session, buf: &mut [u8]) -> Result<Option<Opened>, Box<dyn Error>> {
    let ladder: Vec<usize> = match std::env::var("RAKNET_MTU") {
        Ok(v) => vec![v.parse()?],
        Err(_) => MTU_LADDER.to_vec(),
    };

    for mtu in ladder {
        let request = encode_request_1(mtu).ok_or("MTU below the minimum packet size")?;
        println!("-> OpenConnectionRequest1  mtu={mtu} protocol={PROTOCOL_VERSION}");
        session.socket.send_to(&request, session.server)?;

        let Ok((len, _)) = session.socket.recv_from(buf) else {
            println!("   no reply, dropping to the next MTU\n");
            continue;
        };
        let raw = &buf[..len];

        match raw.first() {
            Some(&ID_OPEN_CONNECTION_REPLY_1) => {
                let reply = decode_reply_1(raw)?;
                println!("<- OpenConnectionReply1  mtu={}", reply.mtu);
                println!(
                    "   cookie {}\n",
                    match reply.cookie {
                        Some(c) => format!("{c:#010x} (security on)"),
                        None => "none (security off)".to_owned(),
                    }
                );
                session.save("reply1.bin", raw)?;
                return Ok(Some(Opened {
                    mtu: reply.mtu,
                    cookie: reply.cookie,
                }));
            }
            Some(&ID_INCOMPATIBLE_PROTOCOL_VERSION) => {
                let incompatible = decode_incompatible(raw)?;
                println!(
                    "<- IncompatibleProtocolVersion: server speaks {}, we sent {PROTOCOL_VERSION}",
                    incompatible.server_protocol
                );
                return Ok(None);
            }
            other => {
                println!("<- unexpected first byte {other:?}");
                return Ok(None);
            }
        }
    }

    println!("no server answered at any MTU");
    Ok(None)
}

/// ConnectionRequest through to Disconnect, acknowledging what arrives.
fn connected(session: &mut Session, buf: &mut [u8]) -> Result<(), Box<dyn Error>> {
    println!("-> ConnectionRequest  (frame set, reliable ordered)");
    let now = session.now();
    session.send_reliable(encode_connection_request(CLIENT_GUID, now))?;

    let mut accepted = false;

    for _ in 0..12 {
        let Ok((len, _)) = session.socket.recv_from(buf) else {
            println!("<- timeout");
            break;
        };
        let raw = &buf[..len];

        let datagram = match Datagram::decode(raw) {
            Ok(d) => d,
            Err(e) => {
                println!("<- undecodable datagram ({len} bytes): {e}");
                continue;
            }
        };

        let set = match datagram {
            Datagram::Ack(ack) => {
                println!("<- ACK {:?}", ack.ranges);
                continue;
            }
            Datagram::Nack(nack) => {
                println!("<- NACK {:?}", nack.ranges);
                continue;
            }
            Datagram::FrameSet(set) => set,
        };

        session.ack(set.sequence)?;

        for frame in &set.frames {
            match frame.payload.first() {
                Some(&ID_CONNECTION_REQUEST_ACCEPTED) => {
                    let reply = decode_connection_request_accepted(&frame.payload)?;
                    println!(
                        "<- ConnectionRequestAccepted  seq={} {} bytes",
                        set.sequence,
                        frame.payload.len()
                    );
                    println!("   system index      {}", reply.system_index);
                    println!("   address slots     {}", reply.system_addresses.len());
                    println!(
                        "   empty slots       {}",
                        reply
                            .system_addresses
                            .iter()
                            .filter(|a| address::is_empty_slot(**a))
                            .count()
                    );
                    println!("   round trip echo   {} ms", reply.request_time);
                    session.save("connection-request-accepted.bin", &frame.payload)?;

                    println!(
                        "-> NewIncomingConnection  (mirroring {} slots)",
                        reply.system_addresses.len()
                    );
                    session.send_reliable(encode_new_incoming_connection(
                        session.server,
                        reply.system_addresses.len(),
                        reply.request_time,
                        reply.accepted_time,
                    ))?;

                    println!("-> ConnectedPing");
                    let now = session.now();
                    session.send_reliable(encode_connected_ping(now))?;
                    accepted = true;
                }
                Some(&ID_CONNECTED_PONG) => {
                    let pong = decode_connected_pong(&frame.payload)?;
                    println!(
                        "<- ConnectedPong  our clock {} ms, server clock {} ms",
                        pong.ping_time, pong.pong_time
                    );
                    session.save("connected-pong.bin", &frame.payload)?;
                    println!("\nconnection established");
                    println!("-> Disconnect");
                    session.send_reliable(encode_disconnect())?;
                    return Ok(());
                }
                Some(&ID_CONNECTED_PING) => {
                    println!("<- ConnectedPing from server");
                }
                other => println!(
                    "<- frame with first byte {other:?}, {} bytes",
                    frame.payload.len()
                ),
            }
        }
    }

    if accepted {
        println!("\naccepted, but no pong came back");
    }
    session.send_reliable(encode_disconnect())?;
    Ok(())
}
