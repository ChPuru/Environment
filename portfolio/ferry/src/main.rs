use anyhow::Result;
use clap::{Parser, Subcommand};
use std::net::TcpListener;
use std::path::PathBuf;

/// ferry — resumable, integrity-verified file transfer over TCP.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Receive files into a directory (one transfer per connection, forever)
    Recv {
        #[arg(long, default_value_t = 3777)]
        port: u16,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    /// Send a file to host:port; interrupted sends resume automatically
    Send { file: PathBuf, addr: String },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Recv { port, dir } => {
            let listener = TcpListener::bind(("0.0.0.0", port))?;
            eprintln!("ferry: receiving into {} on port {port}", dir.display());
            loop {
                match ferry::serve_once(&listener, &dir) {
                    Ok(p) => eprintln!("received {}", p.display()),
                    Err(e) => eprintln!("transfer failed (resumable): {e}"),
                }
            }
        }
        Cmd::Send { file, addr } => {
            let sent = ferry::send(&file, &addr)?;
            eprintln!("sent {sent} bytes (receiver verifies whole-file BLAKE3)");
            Ok(())
        }
    }
}
