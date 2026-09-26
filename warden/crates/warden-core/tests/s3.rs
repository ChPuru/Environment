//! The S3 backend against reality's stand-in: a mock that VERIFIES every
//! signature, forces list pagination, and clamps ranges like S3 does. Then
//! the whole product — init, backup, restore, verify, drill — over it.

mod common;
use common::*;
use tempfile::tempdir;
use warden_core::backend::s3::{mock::MockS3, S3Backend, S3Config};
use warden_core::backend::{conformance, Backend};
use warden_core::chunker::ChunkerParams;
use warden_core::crypto::KdfParams;
use warden_core::object::Compression;
use warden_core::repo::{InitOptions, ReadBack, Repo};

fn cfg(m: &MockS3, prefix: &str) -> S3Config {
    S3Config {
        endpoint: m.endpoint(),
        bucket: m.bucket.clone(),
        region: m.region.clone(),
        access_key: m.access_key.clone(),
        secret_key: m.secret_key.clone(),
        prefix: prefix.to_string(),
    }
}

#[test]
fn s3_backend_is_conformant() {
    let m = MockS3::start("testbucket", "AKIDEXAMPLE", "secret123", "us-east-1");
    let be = S3Backend::new(cfg(&m, "conf/")).expect("backend");
    let problems = conformance::check(&be);
    assert!(
        problems.is_empty(),
        "contract violations:\n{}",
        problems.join("\n")
    );
}

#[test]
fn wrong_secret_is_rejected_by_signature_verification() {
    let m = MockS3::start("testbucket", "AKIDEXAMPLE", "rightsecret", "us-east-1");
    let mut c = cfg(&m, "");
    c.secret_key = "wrongsecret".into();
    let be = S3Backend::new(c).expect("backend");
    let err = be
        .put(
            warden_core::backend::Kind::Object,
            &warden_core::object::StoredName(warden_core::object::Hash256([7; 32])),
            b"data",
        )
        .expect_err("bad signature must be rejected");
    assert!(err.to_string().contains("403"), "expected 403, got: {err}");
}

#[test]
fn whole_product_over_s3() {
    let m = MockS3::start("warden", "AKIDEXAMPLE", "s3secret", "eu-west-1");
    let td = tempdir().expect("td");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);

    let opts = InitOptions {
        chunker: ChunkerParams::TINY_FOR_TESTS,
        kdf: KdfParams::MIN_FOR_TESTS,
        compression: Compression::Zstd,
        pack_target: 4096, // multi-pack over the wire, pagination guaranteed
    };
    let mut repo = Repo::init_backend(S3Backend::new(cfg(&m, "repo/")).expect("be"), PASS, opts)
        .expect("init over s3");

    let out = repo
        .backup_path_with(&src, ReadBack::All)
        .expect("backup over s3");
    assert!(
        out.readback_problems.is_empty(),
        "{:?}",
        out.readback_problems
    );

    let dest = td.path().join("restored");
    repo.restore_file(&out.snapshot_id, &dest)
        .expect("restore over s3");
    assert_trees_equal(&src, &dest);

    let rep = repo.verify_and_record(true).expect("deep verify over s3");
    assert!(rep.problems.is_empty(), "{:?}", rep.problems);
    assert_eq!(rep.orphan_objects, 0);

    let drill = repo.drill(11).expect("drill over s3");
    assert!(drill.problems.is_empty(), "{:?}", drill.problems);
    assert!(drill.files >= 1);

    // Reopen from scratch: everything persists behind the wire.
    let repo2 =
        Repo::open_backend(S3Backend::new(cfg(&m, "repo/")).expect("be"), PASS).expect("reopen");
    assert_eq!(repo2.snapshots().expect("list").len(), 1);
    assert_eq!(repo2.health_records().expect("records").len(), 3);

    // The writer lock works over S3's If-None-Match too.
    let claim = repo2.lock_writer(3600).expect("lock over s3");
    assert!(repo2.lock_writer(3600).is_err(), "second claim must fail");
    repo2.unlock_writer(claim).expect("unlock");
}
