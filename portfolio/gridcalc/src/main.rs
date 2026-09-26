use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

/// gridcalc — formulas over CSV.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print one cell's computed value
    Eval { file: PathBuf, cell: String },
    /// Write the whole sheet with formulas materialized to values
    Recalc {
        file: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Eval { file, cell } => {
            let sheet = gridcalc::Sheet::from_csv(&std::fs::read_to_string(file)?)?;
            println!("{}", gridcalc::fmt_num(sheet.value(&cell)?));
        }
        Cmd::Recalc { file, out } => {
            let sheet = gridcalc::Sheet::from_csv(&std::fs::read_to_string(file)?)?;
            let values = sheet.to_values_csv()?;
            match out {
                Some(p) => std::fs::write(p, values)?,
                None => print!("{values}"),
            }
        }
    }
    Ok(())
}
