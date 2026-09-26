//! The CLI against an S3 repository: s3:// dispatch, env credentials, and
//! the continuous-verification loop — all over the signature-verifying mock.

use std::process::{Command, Output};
use warden_core::backend::s3::mock::MockS3;

fn warden(mock: &MockS3, repo: &str, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", "pw")
        .env("WARDEN_S3_ACCESS_KEY", &mock.access_key)
        .env("WARDEN_S3_SECRET_KEY", &mock.secret_key)
        .env("WARDEN_S3_REGION", &mock.region)
        .env("WARDEN_S3_ENDPOINT", mock.endpoint())
        .arg("--repo")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn warden")
}

#[test]
fn cli_speaks_s3() {
    let mock = MockS3::start("cli-bucket", "AKIDCLI", "clisecret", "us-east-1");
    let repo = "s3://cli-bucket/team/backups";
    let td = tempfile::tempdir().expect("td");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    std::fs::write(src.join("notes.txt"), b"remote bytes").expect("write");
    std::fs::write(src.join("blob.bin"), vec![5u8; 30 * 1024]).expect("write");

    let out = warden(&mock, repo, &["init"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let out = warden(&mock, repo, &["backup", src.to_str().expect("utf8")]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let snapshot = String::from_utf8_lossy(&out.stdout).trim().to_string();
    assert_eq!(snapshot.len(), 64, "snapshot id on stdout");

    let out = warden(&mock, repo, &["verify", "--deep"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let dest = td.path().join("restored");
    let out = warden(
        &mock,
        repo,
        &["restore", &snapshot, dest.to_str().expect("utf8")],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(
        std::fs::read(dest.join("notes.txt")).expect("read"),
        b"remote bytes"
    );

    // Health over S3: verify + read-back recorded; drill clock disabled.
    let out = warden(&mock, repo, &["health", "--max-drill-age", "0"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("health records: 2"), "{stdout}");

    // Wrong secret: every command fails loudly at the wire.
    let out = Command::new(env!("CARGO_BIN_EXE_warden"))
        .env("WARDEN_PASSPHRASE", "pw")
        .env("WARDEN_S3_ACCESS_KEY", &mock.access_key)
        .env("WARDEN_S3_SECRET_KEY", "burglar")
        .env("WARDEN_S3_REGION", &mock.region)
        .env("WARDEN_S3_ENDPOINT", mock.endpoint())
        .args(["--repo", repo, "list"])
        .output()
        .expect("spawn");
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("403"));
}
