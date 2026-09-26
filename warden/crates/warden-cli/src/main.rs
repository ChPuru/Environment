use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use warden_core::backend::s3::{S3Backend, S3Config};
use warden_core::backend::{Backend, LocalBackend};
use warden_core::health::{staleness, StalePolicy};
use warden_core::repo::{InitOptions, ReadBack, Repo};

/// warden — encrypted, deduplicated snapshots that PROVE themselves.
/// Repository format v0, frozen as a v1 candidate.
///
/// Repositories live on local disk (a path) or any S3-compatible store
/// (s3://bucket or s3://bucket/prefix; credentials via WARDEN_S3_ACCESS_KEY
/// and WARDEN_S3_SECRET_KEY, region via WARDEN_S3_REGION, endpoint via
/// WARDEN_S3_ENDPOINT for MinIO and friends).
#[derive(Parser)]
#[command(name = "warden", version, about)]
struct Cli {
    /// Repository: a local path, or s3://bucket[/prefix]
    #[arg(short, long, global = true)]
    repo: Option<String>,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new repository (the S3 bucket itself must already exist)
    Init,
    /// Back up a path; prints the snapshot id. After commit, a sample of
    /// what was just written is read back through the full verification
    /// ladder and the result is recorded in the health log.
    Backup {
        path: PathBuf,
        /// Read-back-after-write: none | sample | all (default: sample)
        #[arg(long, default_value = "sample")]
        readback: String,
        /// Writer-lock time-to-live in seconds (expired locks are stolen)
        #[arg(long, default_value_t = 3600)]
        lock_ttl: u64,
    },
    /// Restore a snapshot to a destination path that must not exist
    Restore { snapshot: String, dest: PathBuf },
    /// List snapshots (id, created, files, size, source path)
    List,
    /// Keyless integrity scrub: rehash every stored file against its name.
    /// Needs no passphrase — run it from anywhere, even the storage box.
    /// By design it writes no health record (recording requires keys).
    Scrub {
        /// Remove staging leftovers in tmp/ (local repositories only)
        #[arg(long)]
        clean_tmp: bool,
    },
    /// Keyed verification of everything snapshots depend on; the outcome is
    /// appended to the health log unless --no-record.
    Verify {
        /// Fetch and fully re-check every reachable object (slow, thorough)
        #[arg(long)]
        deep: bool,
        #[arg(long)]
        no_record: bool,
    },
    /// Restore-drill: restore a few randomly chosen files from a randomly
    /// chosen snapshot into scratch space, verify, clean up, record.
    Drill {
        /// Deterministic selection seed (default: derived from time)
        #[arg(long)]
        seed: Option<u64>,
    },
    /// Delete snapshots by id and/or keep only the newest N. Chunks are
    /// reclaimed by `prune`, not here.
    Forget {
        /// Snapshot ids to delete
        ids: Vec<String>,
        /// Keep only the newest N snapshots
        #[arg(long)]
        keep_last: Option<usize>,
        /// Writer-lock time-to-live in seconds
        #[arg(long, default_value_t = 3600)]
        lock_ttl: u64,
    },
    /// Reclaim space: delete packs no remaining snapshot references and
    /// compact the index. Pack-granularity (no repacking in v0). Refuses
    /// to run if live data is missing. Records a Prune health receipt.
    Prune {
        /// Show the plan without deleting anything
        #[arg(long)]
        dry_run: bool,
        /// Writer-lock time-to-live in seconds
        #[arg(long, default_value_t = 3600)]
        lock_ttl: u64,
    },
    /// Force-release the writer lock. Keyless. Run this ONLY when you are
    /// certain no backup or prune is live (e.g. after a crash); a killed
    /// writer abandons its lock on purpose, and expiry or this command are
    /// the two recovery paths.
    Unlock,
    /// Report what the health log proves and whether it has gone stale.
    /// Exits 1 when problems were found, never-verified, or overdue.
    Health {
        /// Max days since last verify (0 disables this clock)
        #[arg(long, default_value_t = 30)]
        max_verify_age: u64,
        /// Max days since last drill (0 disables this clock)
        #[arg(long, default_value_t = 90)]
        max_drill_age: u64,
    },
}

enum RepoLoc {
    Local(PathBuf),
    S3(S3Config),
}

fn parse_repo(spec: &str) -> Result<RepoLoc> {
    let Some(rest) = spec.strip_prefix("s3://") else {
        return Ok(RepoLoc::Local(PathBuf::from(spec)));
    };
    let (bucket, prefix) = match rest.split_once('/') {
        Some((b, p)) if !p.is_empty() => (b.to_string(), format!("{}/", p.trim_end_matches('/'))),
        Some((b, _)) => (b.to_string(), String::new()),
        None => (rest.to_string(), String::new()),
    };
    if bucket.is_empty() {
        bail!("s3 repo needs a bucket: s3://bucket[/prefix]");
    }
    let access_key = std::env::var("WARDEN_S3_ACCESS_KEY")
        .context("set WARDEN_S3_ACCESS_KEY for s3:// repositories")?;
    let secret_key = std::env::var("WARDEN_S3_SECRET_KEY")
        .context("set WARDEN_S3_SECRET_KEY for s3:// repositories")?;
    let region = std::env::var("WARDEN_S3_REGION").unwrap_or_else(|_| "us-east-1".into());
    let endpoint = std::env::var("WARDEN_S3_ENDPOINT")
        .unwrap_or_else(|_| format!("https://s3.{region}.amazonaws.com"));
    Ok(RepoLoc::S3(S3Config {
        endpoint,
        bucket,
        region,
        access_key,
        secret_key,
        prefix,
    }))
}

fn open_backend_any(loc: &RepoLoc) -> Result<Box<dyn Backend>> {
    Ok(match loc {
        RepoLoc::Local(p) => Box::new(LocalBackend::open(p)?),
        RepoLoc::S3(cfg) => Box::new(S3Backend::new(cfg.clone())?),
    })
}

/// Client-side freshness pin: the newest (created_unix, snapshot_id) this
/// CLIENT has seen for a repo, keyed by repo id. Lives OUTSIDE the
/// repository on purpose — an in-repo pin would roll back with the repo.
/// Detects rollback on this client; multi-client forgets can false-alarm
/// (documented), and the remediation is one backup or removing the file.
fn pin_path(repo_id_hex: &str) -> PathBuf {
    let base = std::env::var("WARDEN_PIN_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            std::env::var("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| {
                    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
                    PathBuf::from(home).join(".cache")
                })
                .join("warden")
                .join("pins")
        });
    base.join(repo_id_hex)
}

fn read_pin(path: &PathBuf) -> Option<(u64, String)> {
    let s = std::fs::read_to_string(path).ok()?;
    let mut it = s.split_whitespace();
    let created: u64 = it.next()?.parse().ok()?;
    let id = it.next()?.to_string();
    Some((created, id))
}

/// Advance (or clear) the pin to the repo's current newest snapshot.
fn update_pin<B: warden_core::backend::Backend>(repo: &Repo<B>) -> Result<()> {
    let path = pin_path(&repo.repo_id_hex());
    match repo.newest_snapshot()? {
        Some((id, created)) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(
                &path,
                format!(
                    "{created} {id}
"
                ),
            )?;
        }
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

/// Freshness verdict: reasons the repo looks OLDER than this client last
/// saw it. Empty = fresh (or no pin yet).
fn freshness_reasons<B: warden_core::backend::Backend>(repo: &Repo<B>) -> Result<Vec<String>> {
    let path = pin_path(&repo.repo_id_hex());
    let Some((pin_created, pin_id)) = read_pin(&path) else {
        return Ok(Vec::new());
    };
    let newest = repo.newest_snapshot()?;
    let stale = match &newest {
        None => true,
        Some((id, created)) => (*created, id.as_str()) < (pin_created, pin_id.as_str()),
    };
    if stale {
        Ok(vec![format!(
            "possible ROLLBACK: this client last saw snapshot {pin_id} (created {pin_created}),              but the repository's newest is {}; if this is intentional (restored from an older              copy, or another client ran forget), run a backup to advance or remove {}",
            newest
                .map(|(id, c)| format!("{id} (created {c})"))
                .unwrap_or_else(|| "ABSENT (no snapshots)".into()),
            path.display()
        )])
    } else {
        Ok(Vec::new())
    }
}

fn passphrase() -> Result<String> {
    std::env::var("WARDEN_PASSPHRASE")
        .context("set WARDEN_PASSPHRASE (interactive prompting lands with the UX pass)")
}

fn exit_with_problems(n: usize) -> ! {
    // process::exit skips buffered-flush destructors; flush explicitly so
    // piped output is never lost.
    let _ = std::io::stdout().flush();
    let _ = std::io::stderr().flush();
    std::process::exit(if n == 0 { 0 } else { 1 });
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn fmt_age(now: u64, t: Option<u64>) -> String {
    match t {
        None => "never".into(),
        Some(t) => format!("{} days ago", now.saturating_sub(t) / 86_400),
    }
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let spec = cli.repo.clone().context("--repo is required")?;
    let loc = parse_repo(&spec)?;
    match cli.cmd {
        Cmd::Init => {
            match &loc {
                RepoLoc::Local(p) => {
                    Repo::init_backend(
                        Box::new(LocalBackend::create(p)?) as Box<dyn Backend>,
                        &passphrase()?,
                        InitOptions::default(),
                    )?;
                }
                RepoLoc::S3(_) => {
                    Repo::init_backend(
                        open_backend_any(&loc)?,
                        &passphrase()?,
                        InitOptions::default(),
                    )?;
                }
            }
            eprintln!("initialized repository at {spec}");
        }
        Cmd::Backup {
            path,
            readback,
            lock_ttl,
        } => {
            let rb = match readback.as_str() {
                "none" => ReadBack::None,
                "sample" => ReadBack::Sample(5),
                "all" => ReadBack::All,
                other => bail!("--readback must be none|sample|all, got {other}"),
            };
            // Lock BEFORE the expensive key unlock: fail fast when locked.
            let claim = {
                let be = open_backend_any(&loc)?;
                warden_core::lock::acquire(&be, lock_ttl, now_unix())?
            };
            let mut repo = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?)?;
            let result = repo.backup_path_with(&path, rb);
            // Release even when backup failed; the claim must not linger.
            warden_core::lock::release(&open_backend_any(&loc)?, claim)?;
            let outcome = result?;
            update_pin(&repo)?;
            for w in &outcome.warnings {
                eprintln!("warning: {w}");
            }
            println!("{}", outcome.snapshot_id);
            if !outcome.readback_problems.is_empty() {
                for p in &outcome.readback_problems {
                    eprintln!("READ-BACK PROBLEM: {p}");
                }
                eprintln!("snapshot committed but read-back FAILED — verify this repo now");
                exit_with_problems(outcome.readback_problems.len());
            }
        }
        Cmd::Restore { snapshot, dest } => {
            let repo = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?)?;
            repo.restore_file(&snapshot, &dest)?;
            eprintln!("restored to {}", dest.display());
        }
        Cmd::List => {
            let repo = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?)?;
            for s in repo.snapshots()? {
                println!(
                    "{}  {}  {} files  {} bytes  {}",
                    s.id, s.created_unix, s.file_count, s.total_len, s.source_path
                );
            }
        }
        Cmd::Scrub { clean_tmp } => {
            let be = open_backend_any(&loc)?;
            let rep = warden_core::verify::scrub(&be)?;
            for p in &rep.problems {
                eprintln!("PROBLEM: {p}");
            }
            if clean_tmp {
                match &loc {
                    RepoLoc::Local(p) => {
                        let n = LocalBackend::open(p)?.clean_tmp()?;
                        eprintln!("removed {n} staging leftovers");
                    }
                    RepoLoc::S3(_) => bail!("--clean-tmp applies to local repositories only"),
                }
            }
            println!(
                "scrubbed {} stored files, {} problems",
                rep.checked,
                rep.problems.len()
            );
            exit_with_problems(rep.problems.len());
        }
        Cmd::Verify { deep, no_record } => {
            let repo = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?)?;
            let rep = if no_record {
                repo.verify(deep)?
            } else {
                repo.verify_and_record(deep)?
            };
            for p in &rep.problems {
                eprintln!("PROBLEM: {p}");
            }
            // Freshness: integrity says the bytes are right; the pin says
            // they are also CURRENT. A violated pin never self-advances —
            // that would silence the very alarm it exists to raise.
            let fresh = freshness_reasons(&repo)?;
            for r in &fresh {
                eprintln!("PROBLEM: {r}");
            }
            if fresh.is_empty() {
                update_pin(&repo)?;
            }
            println!(
                "verified {} snapshots, {} trees, {} chunks; {} orphan objects; {} problems",
                rep.snapshots_checked,
                rep.trees_checked,
                rep.chunks_checked,
                rep.orphan_objects,
                rep.problems.len() + fresh.len()
            );
            exit_with_problems(rep.problems.len() + fresh.len());
        }
        Cmd::Drill { seed } => {
            let repo = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?)?;
            let rep = repo.drill(seed.unwrap_or_else(now_unix))?;
            for p in &rep.problems {
                eprintln!("DRILL PROBLEM: {p}");
            }
            println!(
                "drilled snapshot {}: {} files, {} bytes restored and verified",
                rep.snapshot, rep.files, rep.bytes
            );
            exit_with_problems(rep.problems.len());
        }
        Cmd::Forget {
            ids,
            keep_last,
            lock_ttl,
        } => {
            let claim = {
                let be = open_backend_any(&loc)?;
                warden_core::lock::acquire(&be, lock_ttl, now_unix())?
            };
            let result = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?)
                .and_then(|r| r.forget(&ids, keep_last).map(|gone| (r, gone)));
            warden_core::lock::release(&open_backend_any(&loc)?, claim)?;
            let (repo, gone) = result?;
            // Forgetting the newest snapshot legitimately makes the repo
            // "older": advance the pin so verify does not cry rollback.
            update_pin(&repo)?;
            for id in &gone {
                println!("forgot {id}");
            }
            eprintln!(
                "{} snapshot(s) deleted; run `prune` to reclaim space",
                gone.len()
            );
        }
        Cmd::Prune { dry_run, lock_ttl } => {
            let claim = {
                let be = open_backend_any(&loc)?;
                warden_core::lock::acquire(&be, lock_ttl, now_unix())?
            };
            let repo = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?);
            let result = repo.and_then(|mut r| r.prune(dry_run));
            warden_core::lock::release(&open_backend_any(&loc)?, claim)?;
            let rep = result?;
            let verb = if rep.dry_run {
                "would delete"
            } else {
                "deleted"
            };
            println!(
                "{verb} {} pack(s), reclaiming {} bytes; {} pack(s) kept for {} snapshot(s)",
                rep.packs_deleted, rep.bytes_reclaimed, rep.packs_kept, rep.snapshots_remaining
            );
        }
        Cmd::Unlock => {
            let be = open_backend_any(&loc)?;
            match warden_core::lock::read_holder_info(&be)? {
                Some((pid, expires)) => {
                    eprintln!("removing writer lock held by pid {pid} (expires {expires})")
                }
                None => eprintln!("no writer lock present"),
            }
            be.delete_meta(warden_core::lock::LOCK_PATH)?;
        }
        Cmd::Health {
            max_verify_age,
            max_drill_age,
        } => {
            let repo = Repo::open_backend(open_backend_any(&loc)?, &passphrase()?)?;
            let st = repo.health_status()?;
            let now = now_unix();
            println!("health records: {}", st.records);
            println!("last deep verify: {}", fmt_age(now, st.last_deep_verify));
            println!("last fast verify: {}", fmt_age(now, st.last_fast_verify));
            println!("last read-back:   {}", fmt_age(now, st.last_readback));
            println!("last drill:       {}", fmt_age(now, st.last_drill));
            println!("last prune:       {}", fmt_age(now, st.last_prune));
            println!("latest problems:  {}", st.latest_problems_total);
            for p in &st.latest_problems_sample {
                println!("  - {p}");
            }
            let policy = StalePolicy {
                verify_days: max_verify_age,
                drill_days: max_drill_age,
            };
            let reasons = staleness(&st, &policy, now);
            for r in &reasons {
                eprintln!("STALE: {r}");
            }
            exit_with_problems(reasons.len());
        }
    }
    Ok(())
}
