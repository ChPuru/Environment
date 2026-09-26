use anyhow::{bail, Result};
use clap::Parser;
use std::io::Read;

/// shapeshift — convert CSV/TSV <-> JSON on stdin/stdout.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// csv2json | json2csv
    mode: String,
    /// Field delimiter for the CSV side
    #[arg(long, default_value = ",")]
    delimiter: String,
    /// Infer numbers/bools/nulls instead of keeping strings (csv2json)
    #[arg(long)]
    infer: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let delim = *cli.delimiter.as_bytes().first().unwrap_or(&b',');
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    match cli.mode.as_str() {
        "csv2json" => {
            let v = shapeshift::csv_to_json(&input, delim, cli.infer)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
        }
        "json2csv" => {
            let v: serde_json::Value = serde_json::from_str(&input)?;
            print!("{}", shapeshift::json_to_csv(&v, delim)?);
        }
        other => bail!("mode must be csv2json or json2csv, got {other}"),
    }
    Ok(())
}
