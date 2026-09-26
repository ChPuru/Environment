use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// icalx — list and expand events from an .ics file.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    file: PathBuf,
    /// Window start, YYYYMMDD (default: epoch)
    #[arg(long)]
    from: Option<String>,
    /// Window end, YYYYMMDD (default: from + 365 days)
    #[arg(long)]
    to: Option<String>,
}

fn day(s: &str) -> Result<i64> {
    anyhow::ensure!(s.len() == 8, "dates are YYYYMMDD");
    Ok(icalx::days_from_civil(s[0..4].parse()?, s[4..6].parse()?, s[6..8].parse()?) * 86_400)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.file)?;
    let from = cli.from.as_deref().map(day).transpose()?.unwrap_or(0);
    let to = cli
        .to
        .as_deref()
        .map(day)
        .transpose()?
        .unwrap_or(from + 365 * 86_400);
    for e in icalx::parse(&text)? {
        for t in icalx::expand(&e, from, to) {
            let (y, m, d) = icalx::civil_from_days(t.div_euclid(86_400));
            let tod = t.rem_euclid(86_400);
            println!(
                "{y:04}-{m:02}-{d:02} {:02}:{:02}  {}",
                tod / 3600,
                (tod % 3600) / 60,
                e.summary
            );
        }
    }
    Ok(())
}
