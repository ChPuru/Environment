use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// textract — batch OCR into a searchable JSON index.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// OCR every image under DIR into an index file
    Run {
        dir: PathBuf,
        #[arg(long, default_value = "textract-index.json")]
        out: PathBuf,
    },
    /// Search an index
    Search { index: PathBuf, query: String },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Run { dir, out } => {
            let engine = textract::Tesseract::locate()
                .context("tesseract not found on PATH (set TEXTRACT_TESSERACT to override)")?;
            let idx = textract::run(&engine, &dir)?;
            std::fs::write(&out, serde_json::to_string_pretty(&idx)?)?;
            eprintln!(
                "indexed {} images with {} -> {}",
                idx.docs.len(),
                idx.engine,
                out.display()
            );
        }
        Cmd::Search { index, query } => {
            let idx: textract::Index = serde_json::from_str(&std::fs::read_to_string(index)?)?;
            for h in textract::search(&idx, &query) {
                println!("{}:{}: {}", h.path, h.line, h.snippet);
            }
        }
    }
    Ok(())
}
