//! Scrub/verify through the real binary: exit codes are the scripting
//! contract (0 clean, 1 problems), and scrub must work with NO passphrase.

use std::path::Path;
use std::process::{Command, Output};

fn warden_with_pass(repo: &Path, pass: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", pass)
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn warden")
}

fn warden_no_pass(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_warden"))
        .env_remove("WARDEN_PASSPHRASE")
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn warden")
}

fn first_object(repo: &Path) -> std::path::PathBuf {
    let mut stack = vec![repo.join("objects")];
    while let Some(d) = stack.pop() {
        for ent in std::fs::read_dir(&d).expect("read_dir") {
            let p = ent.expect("ent").path();
            if p.is_dir() {
                stack.push(p);
            } else {
                return p;
            }
        }
    }
    panic!("no objects in repo");
}

#[test]
fn scrub_is_keyless_and_verify_is_honest_about_damage() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    std::fs::write(src.join("a.txt"), b"alpha contents").expect("write");
    std::fs::write(src.join("b.txt"), vec![7u8; 64 * 1024]).expect("write");

    assert!(warden_with_pass(&repo, "pw", &["init"]).status.success());
    let out = warden_with_pass(&repo, "pw", &["backup", src.to_str().expect("utf8")]);
    assert!(
        out.status.success(),
        "backup: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Clean repo: scrub passes WITHOUT any passphrase in the environment.
    let out = warden_no_pass(&repo, &["scrub"]);
    assert!(
        out.status.success(),
        "keyless scrub must pass on a clean repo"
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("0 problems"), "got: {stdout}");

    let out = warden_with_pass(&repo, "pw", &["verify", "--deep"]);
    assert!(
        out.status.success(),
        "deep verify clean: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Inject rot: both tools must fail with exit 1 and NAME the damage.
    let victim = first_object(&repo);
    let original = std::fs::read(&victim).expect("read");
    let mut bad = original.clone();
    let mid = bad.len() / 2;
    bad[mid] ^= 0xff;
    std::fs::write(&victim, &bad).expect("corrupt");

    let out = warden_no_pass(&repo, &["scrub"]);
    assert_eq!(out.status.code(), Some(1), "scrub must exit 1 on rot");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("bit rot"),
        "scrub must name the injury: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = warden_with_pass(&repo, "pw", &["verify", "--deep"]);
    assert_eq!(out.status.code(), Some(1), "deep verify must exit 1 on rot");

    // Heal: clean again.
    std::fs::write(&victim, &original).expect("heal");
    assert!(warden_no_pass(&repo, &["scrub"]).status.success());
    assert!(warden_with_pass(&repo, "pw", &["verify", "--deep"])
        .status
        .success());
}

#[test]
fn scrub_clean_tmp_removes_staging_leftovers() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    assert!(warden_with_pass(&repo, "pw", &["init"]).status.success());
    std::fs::write(repo.join("tmp/stale"), b"junk").expect("plant");
    let out = warden_no_pass(&repo, &["scrub", "--clean-tmp"]);
    assert!(out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("removed 1 staging leftovers"),
        "got: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!repo.join("tmp/stale").exists());
}
