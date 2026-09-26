//! I1 extended to trees: restore(backup(tree)) reproduces structure, bytes,
//! names (including non-UTF-8), permission bits, and mtimes — via a full
//! reopen. Plus randomized tree shapes under proptest.

mod common;
use common::*;
use proptest::prelude::*;
use std::os::unix::ffi::OsStrExt;
use tempfile::tempdir;

#[test]
fn fixture_tree_roundtrips_with_full_fidelity() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);
    // Root dir's own metadata is part of the contract too.
    std::fs::set_permissions(&src, std::fs::Permissions::from_mode(0o751)).expect("chmod");
    set_mtime_for_test(&src, 1_500_000_003);

    let (snap, warnings) = {
        let mut repo = init_tiny(&root);
        let o = repo.backup_path(&src).expect("backup");
        (o.snapshot_id, o.warnings)
    };
    assert!(
        warnings.is_empty(),
        "fixture has no hardlinks/sparse files, got: {warnings:?}"
    );

    let dest = td.path().join("restored");
    open_tiny(&root)
        .restore_file(&snap, &dest)
        .expect("restore");
    assert_trees_equal(&src, &dest);
}

use std::os::unix::fs::PermissionsExt;

#[test]
fn single_symlink_root_roundtrips() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("just-a-link");
    std::os::unix::fs::symlink("some/target", &src).expect("symlink");
    let mut repo = init_tiny(&root);
    let snap = repo.backup_file(&src).expect("backup");
    let dest = td.path().join("restored-link");
    repo.restore_file(&snap, &dest).expect("restore");
    assert_trees_equal(&src, &dest);
}

#[test]
fn list_reports_tree_totals() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);
    let mut repo = init_tiny(&root);
    let snap = repo.backup_path(&src).expect("backup").snapshot_id;
    let infos = repo.snapshots().expect("list");
    let info = infos
        .iter()
        .find(|s| s.id == snap)
        .expect("snapshot listed");
    assert_eq!(info.file_count, 5, "fixture has 5 regular files");
    assert!(info.total_len > 43 * 1024, "totals should cover file bytes");
}

/// Random tree: (depth-bounded) map of paths to either file bytes or dirs.
type TreeSpec = Vec<(Vec<Vec<u8>>, Option<Vec<u8>>)>;

fn arb_tree() -> impl Strategy<Value = TreeSpec> {
    // Each element: (path components, Some(file bytes) | None = dir)
    let name = prop_oneof![
        "[a-z]{1,6}".prop_map(|s| s.into_bytes()),
        Just(b"caf\xE9".to_vec()), // recurring non-UTF-8 name
    ];
    let path = proptest::collection::vec(name, 1..3);
    let body = proptest::option::of(proptest::collection::vec(any::<u8>(), 0..1500));
    proptest::collection::vec((path, body), 0..8)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(12))]
    #[test]
    fn prop_random_trees_roundtrip(spec in arb_tree()) {
        let td = tempdir().expect("td");
        let root = td.path().join("repo");
        let src = td.path().join("src");
        std::fs::create_dir(&src).expect("mkdir");
        // Materialize the spec; later entries may nest under earlier dirs or
        // collide — collisions just overwrite, which is fine for the property.
        for (comps, body) in &spec {
            let mut p = src.clone();
            for c in comps {
                p = p.join(std::ffi::OsStr::from_bytes(c));
            }
            // Random paths can conflict (file where a dir landed, nesting
            // under a file). Skip conflicting entries; the property is about
            // whatever tree actually materialized.
            match body {
                Some(bytes) => {
                    if let Some(parent) = p.parent() {
                        if std::fs::create_dir_all(parent).is_err() {
                            continue;
                        }
                    }
                    if std::fs::metadata(&p).map(|m| m.is_dir()).unwrap_or(false) {
                        continue;
                    }
                    if std::fs::write(&p, bytes).is_err() {
                        continue;
                    }
                }
                None => {
                    let _ = std::fs::create_dir_all(&p);
                }
            }
        }
        let snap = init_tiny(&root).backup_path(&src).expect("backup").snapshot_id;
        let dest = td.path().join("restored");
        open_tiny(&root).restore_file(&snap, &dest).expect("restore");
        assert_trees_equal(&src, &dest);
    }
}
