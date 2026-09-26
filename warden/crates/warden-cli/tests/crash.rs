//! I5 with a real dying process: SIGKILL a live `warden backup` at measured
//! fractions of a full run, then audit. The invariant is exactly this:
//! committed snapshots stay perfect; the in-flight one is absent or complete.
//! (Every *prefix* is covered deterministically in core's crash_protocol
//! test; this adds the reality of buffers, fsync, and an actual dead PID.)

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const PASS: &str = "pw";

fn warden(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", PASS)
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn warden")
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).expect("mkdir dst");
    for ent in std::fs::read_dir(src).expect("read_dir") {
        let ent = ent.expect("ent");
        let from = ent.path();
        let to = dst.join(ent.file_name());
        if ent.file_type().expect("ft").is_dir() {
            copy_tree(&from, &to);
        } else {
            std::fs::copy(&from, &to).expect("copy");
        }
    }
}

fn assert_same_bytes(a: &Path, b: &Path) {
    for name in ["big.bin", "small.txt"] {
        assert_eq!(
            std::fs::read(a.join(name)).expect("read a"),
            std::fs::read(b.join(name)).expect("read b"),
            "content mismatch for {name}"
        );
    }
}

#[test]
fn sigkill_mid_backup_never_damages_committed_state() {
    let td = tempfile::tempdir().expect("td");
    let repo = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    let mut big = vec![0u8; 6 * 1024 * 1024];
    for (i, b) in big.iter_mut().enumerate() {
        *b = (i * 31 % 251) as u8;
    }
    std::fs::write(src.join("big.bin"), &big).expect("write");
    std::fs::write(src.join("small.txt"), b"hello").expect("write");
    let orig = td.path().join("orig");
    copy_tree(&src, &orig);

    assert!(warden(&repo, &["init"]).status.success());

    // Timed full backup -> committed snapshot S1.
    let t0 = Instant::now();
    let out = warden(&repo, &["backup", src.to_str().expect("utf8")]);
    let full = t0.elapsed();
    assert!(
        out.status.success(),
        "baseline backup: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let s1 = String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_string();

    // Mutate so the next backup has real chunk work.
    for b in big.iter_mut().take(3 * 1024 * 1024) {
        *b = b.wrapping_add(1);
    }
    std::fs::write(src.join("big.bin"), &big).expect("rewrite");

    for frac in [0.2f64, 0.5, 0.9] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_warden"))
            .env("WARDEN_PASSPHRASE", PASS)
            .arg("--repo")
            .arg(&repo)
            .args(["backup", src.to_str().expect("utf8")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn backup");
        std::thread::sleep(Duration::from_secs_f64(full.as_secs_f64() * frac));
        let _ = child.kill(); // SIGKILL on unix; may race a natural exit
        let _ = child.wait();

        // Audit the wreckage.
        // A SIGKILLed writer abandons its lock BY DESIGN; the operator
        // protocol after a crash is `warden unlock` (or waiting out the
        // ttl). The harness follows the protocol.
        assert!(warden(&repo, &["unlock"]).status.success());
        let out = warden(&repo, &["scrub"]);
        assert!(
            out.status.success(),
            "frac {frac}: crash left rot?! {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = warden(&repo, &["verify", "--deep"]);
        assert!(
            out.status.success(),
            "frac {frac}: deep verify after crash: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let out = warden(&repo, &["list"]);
        assert!(out.status.success());
        let listing = String::from_utf8_lossy(&out.stdout).to_string();
        let ids: Vec<&str> = listing
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .collect();
        assert!(
            ids.contains(&s1.as_str()),
            "frac {frac}: committed S1 vanished:\n{listing}"
        );
        assert!(
            ids.len() <= 2,
            "frac {frac}: impossible snapshot count:\n{listing}"
        );

        // Committed restores byte-identical to the ORIGINAL tree.
        let dest = td
            .path()
            .join(format!("restore-s1-{}", (frac * 100.0) as u32));
        let out = warden(&repo, &["restore", &s1, dest.to_str().expect("utf8")]);
        assert!(out.status.success(), "frac {frac}: S1 restore failed");
        assert_same_bytes(&orig, &dest);

        // If the kill landed post-commit, the new snapshot must be COMPLETE.
        if let Some(s2) = ids.iter().find(|id| **id != s1.as_str()) {
            let dest = td
                .path()
                .join(format!("restore-s2-{}", (frac * 100.0) as u32));
            let out = warden(&repo, &["restore", s2, dest.to_str().expect("utf8")]);
            assert!(
                out.status.success(),
                "frac {frac}: post-commit snapshot must be complete"
            );
            assert_same_bytes(&src, &dest);
        }
    }

    // Undisturbed second backup commits and restores the mutated tree.
    let out = warden(&repo, &["backup", src.to_str().expect("utf8")]);
    assert!(out.status.success());
    let s_final = String::from_utf8(out.stdout)
        .expect("utf8")
        .trim()
        .to_string();
    let dest = td.path().join("restore-final");
    assert!(
        warden(&repo, &["restore", &s_final, dest.to_str().expect("utf8")])
            .status
            .success()
    );
    assert_same_bytes(&src, &dest);
}
