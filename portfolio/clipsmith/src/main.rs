use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// clipsmith — probe, trim, and thumbnail videos via ffmpeg.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Duration and streams of a file
    Probe { input: PathBuf },
    /// Lossless cut [start, end) seconds (stream copy)
    Trim {
        input: PathBuf,
        start: f64,
        end: f64,
        out: PathBuf,
    },
    /// One frame at a timestamp
    Thumb {
        input: PathBuf,
        at: f64,
        out: PathBuf,
    },
}

fn main() -> Result<()> {
    let tool = clipsmith::Tool::locate()
        .context("ffmpeg/ffprobe not found (PATH or CLIPSMITH_FFMPEG/FFPROBE)")?;
    match Cli::parse().cmd {
        Cmd::Probe { input } => {
            let info = tool.probe(&input)?;
            println!("duration: {:.3}s", info.duration_s);
            for s in info.streams {
                println!("stream: {s}");
            }
        }
        Cmd::Trim {
            input,
            start,
            end,
            out,
        } => {
            tool.trim(&input, start, end, &out)?;
            eprintln!("wrote {}", out.display());
        }
        Cmd::Thumb { input, at, out } => {
            tool.thumbnail(&input, at, &out)?;
            eprintln!("wrote {}", out.display());
        }
    }
    Ok(())
}
