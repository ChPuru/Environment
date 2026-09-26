use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// mirrorer — make DST an exact one-way copy of SRC (hash-verified).
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    src: PathBuf,
    dst: PathBuf,
    /// Also delete files that exist only in DST
    #[arg(long)]
    delete: bool,
    /// Print the plan and change nothing
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let plan = mirrorer::plan(&cli.src, &cli.dst)?;
    for p in &plan.copy {
        println!("copy   {}", p.display());
    }
    for p in &plan.delete {
        println!(
            "{} {}",
            if cli.delete { "delete" } else { "extra " },
            p.display()
        );
    }
    if cli.dry_run {
        eprintln!(
            "dry-run: {} copies, {} deletions planned",
            plan.copy.len(),
            plan.delete.len()
        );
        return Ok(());
    }
    let (c, d) = mirrorer::apply(&cli.src, &cli.dst, &plan, cli.delete)?;
    eprintln!("copied {c}, deleted {d}");
    Ok(())
}
