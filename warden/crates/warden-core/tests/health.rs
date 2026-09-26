//! Continuous verification behaviors: everything records, records compute
//! status, read-back and drills detect real injuries through real read paths.

mod common;
use common::*;
use tempfile::tempdir;
use warden_core::object::HealthEvent;
use warden_core::repo::ReadBack;

struct Fx {
    _td: tempfile::TempDir,
    root: std::path::PathBuf,
    src: std::path::PathBuf,
}

fn fx() -> Fx {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);
    Fx { _td: td, root, src }
}

fn corrupt_every_object(root: &std::path::Path) {
    for (p, _) in object_files_with_sizes(root) {
        let mut b = std::fs::read(&p).expect("read");
        let mid = b.len() / 2;
        b[mid] ^= 0xff;
        std::fs::write(&p, &b).expect("write");
    }
}

#[test]
fn verify_and_record_appends_and_status_reflects_it() {
    let f = fx();
    let mut repo = init_tiny(&f.root);
    repo.backup_path(&f.src).expect("backup");

    let rep = repo.verify_and_record(true).expect("deep");
    assert!(rep.problems.is_empty());
    let rep = repo.verify_and_record(false).expect("fast");
    assert!(rep.problems.is_empty());

    let recs = repo.health_records().expect("records");
    assert_eq!(recs.len(), 2);
    assert!(
        recs.windows(2).all(|w| w[0].time_unix <= w[1].time_unix),
        "oldest first"
    );
    let st = repo.health_status().expect("status");
    assert!(st.last_deep_verify.is_some() && st.last_fast_verify.is_some());
    assert_eq!(st.latest_problems_total, 0);
    assert!(st.last_drill.is_none());
}

#[test]
fn readback_all_is_clean_on_healthy_write_and_recorded() {
    let f = fx();
    let mut repo = init_tiny(&f.root);
    let out = repo
        .backup_path_with(&f.src, ReadBack::All)
        .expect("backup");
    assert!(
        out.readback_problems.is_empty(),
        "{:?}",
        out.readback_problems
    );
    assert!(!out.new_chunks.is_empty() && !out.new_trees.is_empty());

    let recs = repo.health_records().expect("records");
    assert_eq!(recs.len(), 1);
    match &recs[0].event {
        HealthEvent::ReadBack {
            sampled_chunks,
            sampled_trees,
            of_chunks,
            of_trees,
        } => {
            assert_eq!(*sampled_chunks, out.new_chunks.len() as u64);
            assert_eq!(*sampled_trees, out.new_trees.len() as u64);
            assert_eq!(*of_chunks, out.new_chunks.len() as u64);
            assert_eq!(*of_trees, out.new_trees.len() as u64);
        }
        other => panic!("expected ReadBack record, got {other:?}"),
    }
    assert_eq!(recs[0].problems_total, 0);

    // Sample mode: at least one of each, never more than all, still clean.
    let src2 = f.root.parent().unwrap().join("src2");
    std::fs::create_dir(&src2).expect("mkdir");
    write_file(&src2.join("a.bin"), &deterministic_bytes(20_000, 5));
    let out = repo
        .backup_path_with(&src2, ReadBack::Sample(10))
        .expect("backup2");
    assert!(out.readback_problems.is_empty());
    let recs = repo.health_records().expect("records");
    match &recs[1].event {
        HealthEvent::ReadBack {
            sampled_chunks,
            sampled_trees,
            of_chunks,
            of_trees,
        } => {
            assert!(*sampled_chunks >= 1 && sampled_chunks <= of_chunks);
            assert!(*sampled_trees >= 1 && sampled_trees <= of_trees);
        }
        other => panic!("expected ReadBack record, got {other:?}"),
    }
}

#[test]
fn read_back_detects_rot_through_real_read_paths() {
    let f = fx();
    let mut repo = init_tiny(&f.root);
    let out = repo
        .backup_path_with(&f.src, ReadBack::None)
        .expect("backup");
    assert!(
        out.readback_problems.is_empty(),
        "None mode must not read back"
    );
    assert_eq!(
        repo.health_records().expect("records").len(),
        0,
        "None mode records nothing"
    );

    corrupt_every_object(&f.root);
    let problems = repo.read_back(&out.new_chunks, &out.new_trees);
    assert!(!problems.is_empty(), "rot in every pack must surface");
}

#[test]
fn drill_restores_real_files_cleans_up_and_records() {
    let f = fx();
    let mut repo = init_tiny(&f.root);
    repo.backup_path(&f.src).expect("backup");

    let before: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .expect("tmp")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("warden-drill-"))
        .collect();

    let rep = repo.drill(7).expect("drill");
    assert!(rep.problems.is_empty(), "{:?}", rep.problems);
    assert!(rep.files >= 1 && rep.files <= 3);
    assert!(rep.bytes > 0);

    let after: Vec<_> = std::fs::read_dir(std::env::temp_dir())
        .expect("tmp")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("warden-drill-"))
        .collect();
    assert_eq!(
        before.len(),
        after.len(),
        "drill must clean its scratch dir"
    );

    let recs = repo.health_records().expect("records");
    assert!(matches!(
        recs.last().expect("rec").event,
        HealthEvent::Drill { .. }
    ));

    corrupt_every_object(&f.root);
    let rep = repo.drill(7).expect("drill returns a report, not an error");
    assert!(!rep.problems.is_empty(), "drill over rot must scream");
}

#[test]
fn drill_on_empty_repo_is_an_error() {
    let td = tempdir().expect("td");
    let repo = init_tiny(&td.path().join("repo"));
    assert!(repo.drill(1).is_err());
}
