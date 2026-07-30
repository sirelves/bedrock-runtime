//! Server binary: argument parsing, config loading, logging setup, shutdown signals.
//!
//! Game logic in this crate is a bug.

fn main() {
    let arg = std::env::args().nth(1);
    match arg.as_deref() {
        Some("--version" | "-V") => println!("bedrock-runtime {}", env!("CARGO_PKG_VERSION")),
        Some("--help" | "-h") | None => print_help(),
        Some(other) => {
            eprintln!("unknown argument: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn print_help() {
    println!(
        "bedrock-runtime — a Minecraft: Bedrock Edition server in Rust

USAGE:
    bedrock-cli [OPTIONS]

OPTIONS:
    -h, --help       Print this message
    -V, --version    Print the version

STATUS:
    Pre-alpha (M0). No subsystem is implemented yet, so there is nothing to
    connect to. See docs/ROADMAP.md for what has to land first."
    );
}
