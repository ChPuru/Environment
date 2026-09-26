//! I3 over the tree shape: the same exhaustive corruption matrix, now with
//! tree objects, deep files, symlinks, and metadata in the blast radius.

mod common;
use common::*;
use tempfile::tempdir;

#[test]
fn any_single_corruption_is_never_silent_tree() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);

    let snap = {
        let mut repo = init_tiny(&root);
        repo.backup_path(&src).expect("backup").snapshot_id
    };

    tamper_matrix(&root, &snap, td.path(), &|dest| {
        assert_trees_equal(&src, dest)
    });

    let dest = td.path().join("restored-final");
    open_tiny(&root)
        .restore_file(&snap, &dest)
        .expect("pristine restore");
    assert_trees_equal(&src, &dest);
}
