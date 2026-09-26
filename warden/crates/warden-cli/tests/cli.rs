//! End-to-end through the real binary with production-default parameters
//! (full-cost Argon2id, default chunker): the user-facing path, exercised as
//! a user would.

use std::path::Path;
use std::process::{Command, Output};

fn warden(repo: &Path, pass: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", pass)
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn warden")
}

fn deterministic_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.max(1);
    let mut v = Vec::with_capacity(len + 8);
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    v.truncate(len);
    v
}

#[test]
fn end_to_end_init_backup_restore_list() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    let src = td.path().join("src.bin");
    let data = deterministic_bytes(300 * 1024, 11);
    std::fs::write(&src, &data).expect("write src");

    let out = warden(&repo, "pw", &["init"]);
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = warden(&repo, "pw", &["backup", src.to_str().expect("utf8")]);
    assert!(
        out.status.success(),
        "backup failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let snap = String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_string();
    assert_eq!(
        snap.len(),
        64,
        "snapshot id should be 64 hex chars, got: {snap}"
    );

    let dest = td.path().join("restored.bin");
    let out = warden(
        &repo,
        "pw",
        &["restore", &snap, dest.to_str().expect("utf8")],
    );
    assert!(
        out.status.success(),
        "restore failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(&dest).expect("read"),
        data,
        "restored bytes differ"
    );

    let out = warden(&repo, "pw", &["list"]);
    assert!(out.status.success());
    let listing = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(
        listing.contains(&snap),
        "list output missing snapshot id:\n{listing}"
    );
}

#[test]
fn wrong_passphrase_is_rejected_with_nonzero_exit() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    assert!(warden(&repo, "right", &["init"]).status.success());
    let out = warden(&repo, "wrong", &["list"]);
    assert!(!out.status.success(), "wrong passphrase must fail");
    let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
    assert!(
        err.contains("passphrase"),
        "error should name the problem: {err}"
    );
}

#[test]
fn unknown_snapshot_fails_cleanly() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    assert!(warden(&repo, "pw", &["init"]).status.success());
    let bogus = "ab".repeat(32);
    let dest = td.path().join("never.bin");
    let out = warden(
        &repo,
        "pw",
        &["restore", &bogus, dest.to_str().expect("utf8")],
    );
    assert!(!out.status.success());
    assert!(
        !dest.exists(),
        "failed restore must not create the destination"
    );
}
