//! The continuous-verification loop through the real binary: backup records
//! read-back, verify records, drill records, health judges — with the exit
//! codes cron will live by.

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

#[test]
fn the_whole_loop_records_judges_and_screams() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    std::fs::write(src.join("a.txt"), b"alpha").expect("write");
    std::fs::write(src.join("b.bin"), vec![9u8; 48 * 1024]).expect("write");

    assert!(warden(&repo, &["init"]).status.success());

    // Backup with default read-back sampling: succeeds, records health.
    let out = warden(&repo, &["backup", src.to_str().expect("utf8")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Health now: read-back recorded, but never verified/drilled -> exit 1.
    let out = warden(&repo, &["health"]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("never verified"), "{stderr}");
    assert!(stderr.contains("never drilled"), "{stderr}");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("health records: 1"), "{stdout}");

    // Verify (records) + drill (records) -> health goes green.
    assert!(warden(&repo, &["verify", "--deep"]).status.success());
    assert!(warden(&repo, &["drill", "--seed", "3"]).status.success());
    let out = warden(&repo, &["health"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("health records: 3"), "{stdout}");
    assert!(stdout.contains("latest problems:  0"), "{stdout}");

    // Disabled clocks keep a bare repo green too.
    assert!(warden(
        &repo,
        &["health", "--max-verify-age", "0", "--max-drill-age", "0"]
    )
    .status
    .success());

    // Inject rot -> verify screams AND health's latest record goes red.
    let victim = {
        let mut found = None;
        let mut stack = vec![repo.join("objects")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).expect("rd") {
                let p = e.expect("e").path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    found = Some(p);
                }
            }
        }
        found.expect("an object")
    };
    let mut b = std::fs::read(&victim).expect("read");
    let mid = b.len() / 2;
    b[mid] ^= 0xff;
    std::fs::write(&victim, &b).expect("corrupt");

    let out = warden(&repo, &["verify", "--deep"]);
    assert_eq!(out.status.code(), Some(1));
    let out = warden(&repo, &["health", "--max-drill-age", "0"]);
    assert_eq!(
        out.status.code(),
        Some(1),
        "health must reflect the red verify"
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("problems"));
}
