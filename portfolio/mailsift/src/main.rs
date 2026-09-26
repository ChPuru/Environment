use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use mailsift::Query;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// mailsift — stats, search, and export for mbox archives.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    mbox: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Message count and top senders
    Stats,
    /// Filter by sender/subject/date
    Search {
        #[arg(long)]
        from: Option<String>,
        #[arg(long)]
        subject: Option<String>,
        /// Unix seconds lower bound
        #[arg(long)]
        since: Option<i64>,
    },
    /// Write the Nth message (0-based) as .eml to stdout
    Export { index: usize },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let text = std::fs::read_to_string(&cli.mbox)?;
    let msgs = mailsift::parse_mbox(&text)?;
    match cli.cmd {
        Cmd::Stats => {
            println!("{} messages", msgs.len());
            let mut senders: BTreeMap<String, usize> = BTreeMap::new();
            for m in &msgs {
                *senders.entry(m.header("from").to_string()).or_default() += 1;
            }
            let mut v: Vec<_> = senders.into_iter().collect();
            v.sort_by_key(|e| std::cmp::Reverse(e.1));
            for (s, n) in v.into_iter().take(10) {
                println!("{n:5}  {s}");
            }
        }
        Cmd::Search {
            from,
            subject,
            since,
        } => {
            let q = Query {
                from: from.as_deref(),
                subject: subject.as_deref(),
                since_unix: since,
            };
            for m in mailsift::search(&msgs, &q) {
                println!("{}  |  {}", m.header("from"), m.subject());
            }
        }
        Cmd::Export { index } => {
            let m = msgs.get(index).context("no such message")?;
            let body = m
                .raw
                .split_once('\n')
                .map(|(_, rest)| rest)
                .unwrap_or(&m.raw);
            if body.is_empty() {
                bail!("empty message");
            }
            print!("{body}");
        }
    }
    Ok(())
}
