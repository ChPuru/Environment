//! forget + prune through the real binary: keep-last, dry-run honesty,
//! actual reclamation, and a green deep verify after.

use std::path::Path;
use std::process::{Command, Output};

fn warden(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", "pw")
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn warden")
}

fn dir_bytes(root: &Path) -> u64 {
    let mut total = 0;
    let mut stack = vec![root.join("objects")];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).expect("rd") {
            let p = e.expect("e").path();
            if p.is_dir() {
                stack.push(p);
            } else {
                total += std::fs::metadata(&p).expect("meta").len();
            }
        }
    }
    total
}

#[test]
fn forget_prune_reclaims_and_verify_stays_green() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    // Incompressible and BIGGER than the 16 MiB production pack target, so
    // the dead data owns whole packs of its own (v0 prune is pack-granular;
    // a pack sharing one live chunk survives whole — documented).
    let mut junk = vec![0u8; 40 * 1024 * 1024];
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    for b in junk.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    std::fs::write(src.join("old.bin"), &junk).expect("write");
    std::fs::write(src.join("keep.txt"), b"still here").expect("write");

    assert!(warden(&repo, &["init"]).status.success());
    assert!(warden(
        &repo,
        &["backup", src.to_str().expect("utf8"), "--readback", "none"]
    )
    .status
    .success());
    std::fs::remove_file(src.join("old.bin")).expect("rm");
    assert!(warden(
        &repo,
        &["backup", src.to_str().expect("utf8"), "--readback", "none"]
    )
    .status
    .success());

    let before = dir_bytes(&repo);
    let out = warden(&repo, &["forget", "--keep-last", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Dry run: says what it would do, does nothing.
    let out = warden(&repo, &["prune", "--dry-run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("would delete"));
    assert_eq!(dir_bytes(&repo), before, "dry-run must not touch bytes");

    // Real prune: the report's byte count must match disk EXACTLY, and at
    // least one full 16 MiB pack of dead junk must actually be reclaimed.
    let out = warden(&repo, &["prune"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    assert!(stdout.contains("deleted"), "{stdout}");
    let reclaimed: u64 = stdout
        .split("reclaiming ")
        .nth(1)
        .and_then(|s| s.split(' ').next())
        .and_then(|n| n.parse().ok())
        .expect("parse reclaimed bytes from report");
    assert!(
        reclaimed >= 16 * 1024 * 1024,
        "at least one dead pack: {stdout}"
    );
    assert_eq!(
        dir_bytes(&repo),
        before - reclaimed,
        "the receipt must match the disk"
    );

    let out = warden(&repo, &["verify", "--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = warden(&repo, &["list"]);
    assert_eq!(String::from_utf8_lossy(&out.stdout).lines().count(), 1);
}
