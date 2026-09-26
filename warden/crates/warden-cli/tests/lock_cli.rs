//! Two real processes contending for the writer lock: the second fails
//! fast (pre-KDF) with a Locked error; after the first finishes, backups
//! flow again.

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

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
fn concurrent_backup_is_rejected_while_lock_is_held() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    // Big enough that the first backup holds the lock for several seconds
    // (the KDF alone is seconds in a debug build; lock is taken before it).
    std::fs::write(src.join("big.bin"), vec![3u8; 4 * 1024 * 1024]).expect("write");

    assert!(warden(&repo, &["init"]).status.success());

    let mut first = Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", "pw")
        .arg("--repo")
        .arg(&repo)
        .args(["backup", src.to_str().expect("utf8")])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn first backup");
    std::thread::sleep(Duration::from_millis(400)); // first has the lock by now

    let out = warden(&repo, &["backup", src.to_str().expect("utf8")]);
    assert!(
        !out.status.success(),
        "second backup must be rejected while locked"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("locked"), "must say why: {stderr}");

    assert!(
        first.wait().expect("wait").success(),
        "first backup must finish clean"
    );
    let out = warden(&repo, &["backup", src.to_str().expect("utf8")]);
    assert!(
        out.status.success(),
        "lock must be released after: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
