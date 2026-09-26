use anyhow::Result;
use clap::Parser;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};

/// relay — tiny TCP chat. `serve` runs a hub; `join` is a terminal client.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// serve | join
    mode: String,
    /// serve: port to bind; join: host:port
    target: String,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.mode.as_str() {
        "serve" => {
            let listener = TcpListener::bind(format!("0.0.0.0:{}", cli.target))?;
            eprintln!("relay hub on port {}", cli.target);
            relay::serve(listener);
        }
        "join" => {
            let stream = TcpStream::connect(&cli.target)?;
            let mut w = stream.try_clone()?;
            let r = BufReader::new(stream);
            std::thread::spawn(move || {
                for line in r.lines() {
                    match line {
                        Ok(l) => println!("{l}"),
                        Err(_) => break,
                    }
                }
                std::process::exit(0);
            });
            for line in std::io::stdin().lock().lines() {
                w.write_all(line?.as_bytes())?;
                w.write_all(b"\n")?;
            }
        }
        other => anyhow::bail!("mode must be serve|join, got {other}"),
    }
    Ok(())
}
