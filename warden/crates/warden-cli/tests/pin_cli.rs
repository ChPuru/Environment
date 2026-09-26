//! The freshness pin against an actual rollback: the whole repository
//! swapped for an older-but-perfectly-valid copy. Integrity is green by
//! construction; the pin is what screams.

use std::path::Path;
use std::process::{Command, Output};

fn warden(pin_dir: &Path, repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", "pw")
        .env("WARDEN_PIN_DIR", pin_dir)
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn warden")
}

fn cp_r(from: &Path, to: &Path) {
    let st = Command::new("cp")
        .arg("-a")
        .arg(from)
        .arg(to)
        .status()
        .expect("cp");
    assert!(st.success());
}

#[test]
fn a_rolled_back_repository_fails_verify_until_advanced() {
    let td = tempfile::tempdir().expect("td");
    let pins = td.path().join("pins");
    let repo = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    std::fs::write(src.join("v1.txt"), b"first era").expect("write");

    assert!(warden(&pins, &repo, &["init"]).status.success());
    assert!(warden(
        &pins,
        &repo,
        &["backup", src.to_str().expect("utf8"), "--readback", "none"]
    )
    .status
    .success());

    // Freeze an older-but-valid copy of the whole repository.
    let old_copy = td.path().join("repo-old");
    cp_r(&repo, &old_copy);

    // Move forward: second snapshot, verify pins it.
    std::thread::sleep(std::time::Duration::from_millis(1100)); // distinct created_unix
    std::fs::write(src.join("v2.txt"), b"second era").expect("write");
    assert!(warden(
        &pins,
        &repo,
        &["backup", src.to_str().expect("utf8"), "--readback", "none"]
    )
    .status
    .success());
    let out = warden(&pins, &repo, &["verify", "--deep", "--no-record"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // ROLLBACK: swap in the old copy. Every hash checks. The pin knows.
    std::fs::remove_dir_all(&repo).expect("rm");
    std::fs::rename(&old_copy, &repo).expect("mv");
    let out = warden(&pins, &repo, &["verify", "--deep", "--no-record"]);
    assert_eq!(out.status.code(), Some(1), "rollback must fail verify");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ROLLBACK"), "{stderr}");

    // The alarm must not self-clear: verify again, still screaming.
    let out = warden(&pins, &repo, &["verify", "--no-record"]);
    assert_eq!(out.status.code(), Some(1), "pin must not self-advance");

    // Remediation path 1: a new backup advances history past the pin.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    std::fs::write(src.join("v3.txt"), b"third era").expect("write");
    assert!(warden(
        &pins,
        &repo,
        &["backup", src.to_str().expect("utf8"), "--readback", "none"]
    )
    .status
    .success());
    let out = warden(&pins, &repo, &["verify", "--no-record"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Legit "older" case: forgetting the newest on THIS client re-pins.
    let out = warden(&pins, &repo, &["forget", "--keep-last", "1"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = warden(&pins, &repo, &["verify", "--no-record"]);
    assert!(
        out.status.success(),
        "forget on this client must not false-alarm: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
