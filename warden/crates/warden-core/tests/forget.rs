//! Snapshot deletion: by id, by keep-policy, and the refusals that keep it
//! from being a footgun.

mod common;
use common::*;
use tempfile::tempdir;

fn three_snapshots(
    root: &std::path::Path,
    base: &std::path::Path,
) -> (
    warden_core::repo::Repo<warden_core::backend::LocalBackend>,
    Vec<String>,
) {
    let mut repo = init_tiny(root);
    let mut ids = Vec::new();
    for i in 0..3u8 {
        let src = base.join(format!("src{i}"));
        std::fs::create_dir(&src).expect("mkdir");
        write_file(
            &src.join("f.bin"),
            &deterministic_bytes(9000 + i as usize * 500, i as u64),
        );
        if i > 0 {
            // keep-last is a TEMPORAL promise; give the clock a tick so
            // "newest" is a fact, not a same-second coin flip.
            std::thread::sleep(std::time::Duration::from_millis(1100));
        }
        ids.push(repo.backup_path(&src).expect("backup").snapshot_id);
    }
    (repo, ids)
}

#[test]
fn forget_by_id_removes_exactly_those_snapshots() {
    let td = tempdir().expect("td");
    let (repo, ids) = three_snapshots(&td.path().join("repo"), td.path());
    let gone = repo.forget(&[ids[1].clone()], None).expect("forget");
    assert_eq!(gone, vec![ids[1].clone()]);
    let left: Vec<String> = repo
        .snapshots()
        .expect("list")
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(left.contains(&ids[0]) && left.contains(&ids[2]) && !left.contains(&ids[1]));
    // Idempotent-ish: forgetting an unknown id is an ERROR (typo safety).
    assert!(repo.forget(&[ids[1].clone()], None).is_err());
}

#[test]
fn keep_last_n_keeps_the_newest_n() {
    let td = tempdir().expect("td");
    let (repo, ids) = three_snapshots(&td.path().join("repo"), td.path());
    let gone = repo.forget(&[], Some(1)).expect("keep-last");
    assert_eq!(gone.len(), 2);
    assert!(gone.contains(&ids[0]) && gone.contains(&ids[1]));
    let left: Vec<String> = repo
        .snapshots()
        .expect("list")
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert_eq!(left, vec![ids[2].clone()], "newest survives");
    // keep-last 0 would erase history entirely: refuse.
    assert!(repo.forget(&[], Some(0)).is_err());
    // Remaining snapshot still restores byte-perfect (chunks untouched).
    let dest = td.path().join("restored");
    repo.restore_file(&ids[2], &dest).expect("restore");
    assert_trees_equal(&td.path().join("src2"), &dest);
}
