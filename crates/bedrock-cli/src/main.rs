//! Server binary: argument parsing, the UDP socket, logging, shutdown.
//!
//! The layers below are sans-io (ADR-012), so this is where the loop lives: read a
//! datagram, feed it in, tick, drain what comes out.
//!
//! Game logic in this crate is a bug.

use bedrock_server::server::{Closed, Stage};
use bedrock_server::server::{
    DEFAULT_PORT, Event, Jwks, Server, TARGET_PROTOCOL, TOKEN_KEYS_URL, advertisement,
};
use std::error::Error;
use std::net::UdpSocket;
use std::path::Path;
use std::time::{Duration, Instant};

const GUID: i64 = 0x0bed_0c00_0000_0003;

/// How often the issuer's keys are fetched again.
///
/// Keys rotate, and a server that fetched once at boot starts refusing every login the
/// day they do. An hour is far below any published rotation period and costs one HTTP
/// request.
const KEY_REFRESH: Duration = Duration::from_secs(3600);

struct Options {
    port: u16,
    name: String,
    dump: Option<String>,
    stage: Stage,
}

/// Fetches the issuer's signing keys.
fn fetch_identity_keys() -> Result<Jwks, Box<dyn Error>> {
    let body = ureq::get(TOKEN_KEYS_URL)
        .call()?
        .body_mut()
        .read_to_string()?;
    Ok(Jwks::parse(&body)?)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
        .unwrap_or(0)
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
    server.set_stage(options.stage);

    println!("bedrock-runtime {}", env!("CARGO_PKG_VERSION"));
    println!("listening on  {local}");
    println!("advertising   {advertisement}");
    if let Some(dir) = &options.dump {
        println!("capturing to  {dir}");
    }
    if options.stage != Stage::Spawn {
        println!("parando em    {:?}  (bisseccao)", options.stage);
    }
    println!("ctrl-c to stop\n");

    // Fetched before the socket does anything: a login that arrives before the keys do
    // cannot be verified, and this server refuses what it cannot verify.
    match fetch_identity_keys() {
        Ok(keys) => {
            server.set_identity_keys(keys, unix_now(), Instant::now());
            println!("identidade    chaves do emissor carregadas");
        }
        Err(e) => {
            eprintln!("identidade    NAO foi possivel buscar as chaves: {e}");
            eprintln!("              todo login sera recusado ate a proxima tentativa");
        }
    }
    let mut last_refresh = Instant::now();

    let mut captured = 0usize;
    let mut buf = [0u8; 2048];

    loop {
        let now = Instant::now();

        // Re-read the wall clock every pass. Deriving it from a monotonic instant
        // drifts across suspend, and the symptom is refusing perfectly good logins.
        server.set_clock(unix_now(), now);

        if now.duration_since(last_refresh) >= KEY_REFRESH {
            last_refresh = now;
            match fetch_identity_keys() {
                Ok(keys) => server.set_identity_keys(keys, unix_now(), now),
                Err(e) => eprintln!("identidade    falha ao renovar as chaves: {e}"),
            }
        }
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
        Event::Disconnected(peer, reason) => {
            let (label, meaning) = match reason {
                Closed::ByPeer => ("saiu", "o cliente enviou Disconnect: recusou algo"),
                Closed::Timeout => ("silencio", "20s sem falar: ficou esperando, nao recusou"),
                Closed::Unreachable => ("inalcancavel", "parou de confirmar o que enviamos"),
                Closed::ByUs => ("fechado por nos", ""),
            };
            println!("desconectou   {peer}  [{label}]");
            if !meaning.is_empty() {
                println!("  {meaning}");
            }
        }
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
        Event::LoginAccepted {
            peer,
            client_protocol,
            gamertag,
        } => {
            println!(
                "login OK      {peer}  protocol {client_protocol}, identidade verificada{}",
                gamertag
                    .as_deref()
                    .map(|g| format!(", jogador {g}"))
                    .unwrap_or_default()
            );
            println!("  -> ServerToClientHandshake, encryption starts after this");
        }
        Event::LoginRejected { peer, reason } => {
            println!("login RECUSADO {peer}");
            println!("  {reason}");
        }
        Event::HandshakeAccepted(peer) => {
            println!("HANDSHAKE ACEITO  {peer}");
            println!("  decifrado com sucesso: a derivacao, o IV e o checksum estao certos");
        }
        Event::PlayStatusSent { peer, status } => {
            println!("  -> PlayStatus {status:?}  (cifrado)");
            let _ = peer;
        }
        Event::PacksAnswered { peer, response } => {
            println!("packs         {peer}  cliente respondeu {response:?}");
        }
        Event::ReadyForWorld(peer) => {
            println!("pronto        {peer}  cliente esperando o mundo");
        }
        Event::WorldSent(peer) => {
            println!("  -> StartGame  {peer}  (mundo flat, paleta de blocos vazia)");
        }
        Event::ChunkRadiusGranted {
            peer,
            requested,
            granted,
        } => {
            println!("  -> ChunkRadiusUpdated  {peer}  pediu {requested}, concedido {granted}");
        }
        Event::Spawned { peer, columns } => {
            println!("  -> {columns} colunas + PlayStatus PlayerSpawn  {peer}");
        }
        Event::Decrypted { peer, id, len } => {
            println!("cifrado ok    {peer}  packet {id}, {len} bytes decifrados");
        }
        Event::DecryptionFailed(peer) => {
            println!("FALHA AO DECIFRAR  {peer}");
            println!("  a derivacao, o IV ou a formula do checksum esta errada");
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
        stage: Stage::Spawn,
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
            "--stop-after" => {
                let name = args.next().ok_or("--stop-after needs a value")?;
                options.stage = Stage::parse(&name)
                    .ok_or("--stop-after: start-game, world, radius, chunks or spawn")?;
            }
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
    --stop-after <STAGE>
                     Stop early: start-game, world, radius, chunks, spawn.
                     For bisecting a client that closes on a malformed packet.
    -h, --help       Print this message
    -V, --version    Print the version

STATUS:
    The transport is complete and a real client connects. The login sequence is
    not implemented, so a client connects and then waits. See docs/ROADMAP.md."
    );
}
