use anyhow::Result;
use clap::Parser;
use std::path::PathBuf;

/// shutterbug — organize photos into DST/YYYY/MM by EXIF date.
#[derive(Parser)]
#[command(version, about)]
struct Cli {
    src: PathBuf,
    dst: PathBuf,
    /// Print the plan and move nothing
    #[arg(long)]
    dry_run: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut moved = 0;
    let mut skipped = 0;
    for e in std::fs::read_dir(&cli.src)? {
        let p = e?.path();
        let ext = p
            .extension()
            .and_then(|x| x.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if !(ext == "jpg" || ext == "jpeg") {
            continue;
        }
        match shutterbug::plan_one(&p, &cli.dst)? {
            shutterbug::Action::SkipIdentical => {
                println!("skip   {} (identical already filed)", p.display());
                skipped += 1;
            }
            act @ shutterbug::Action::Move { .. } => {
                if let shutterbug::Action::Move { to, dated_by_exif } = &act {
                    println!(
                        "move   {} -> {}{}",
                        p.display(),
                        to.display(),
                        if *dated_by_exif {
                            ""
                        } else {
                            "  (mtime fallback)"
                        }
                    );
                }
                if !cli.dry_run {
                    shutterbug::apply_one(&p, &act)?;
                }
                moved += 1;
            }
        }
    }
    eprintln!(
        "{moved} filed, {skipped} identical skipped{}",
        if cli.dry_run { " (dry-run)" } else { "" }
    );
    Ok(())
}
