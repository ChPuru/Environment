//! Incremental economics on trees, and the metadata contract's warnings:
//! fidelity degradation (hardlinks stored as copies, sparse stored dense)
//! must be reported, never silent.

mod common;
use common::*;
use std::io::{Seek, SeekFrom, Write};
use std::os::unix::fs::MetadataExt;
use tempfile::tempdir;

#[test]
fn unchanged_tree_rebackup_stores_zero_new_objects() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);

    let mut repo = init_tiny(&root);
    repo.backup_path(&src).expect("first");
    let baseline = count_objects(&root);
    let snaps_before = repo.snapshots().expect("list").len();

    repo.backup_path(&src).expect("second");
    assert_eq!(
        count_objects(&root),
        baseline,
        "re-backup of an unchanged tree must add zero chunk/tree objects"
    );
    assert_eq!(repo.snapshots().expect("list").len(), snaps_before + 1);
}

#[test]
fn one_file_edit_stores_bounded_neighborhood_plus_ancestor_trees() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);

    let mut repo = init_tiny(&root);
    repo.backup_path(&src).expect("first");
    let before: std::collections::HashSet<_> = object_files_with_sizes(&root)
        .into_iter()
        .map(|(p, _)| p)
        .collect();

    // Edit one byte mid-file, three levels deep.
    let target = src.join("sub/deeper/leaf");
    let mut bytes = std::fs::read(&target).expect("read");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(&target, &bytes).expect("write");

    repo.backup_path(&src).expect("second");
    // Pack era: changed chunks + 3 re-encoded ancestor trees land in fresh
    // pack file(s). Locality is a BYTE bound now: a one-byte edit must not
    // re-store more than a small neighborhood.
    let new: Vec<(std::path::PathBuf, u64)> = object_files_with_sizes(&root)
        .into_iter()
        .filter(|(p, _)| !before.contains(p))
        .collect();
    let new_bytes: u64 = new.iter().map(|(_, l)| l).sum();
    assert!(!new.is_empty(), "an edit must store something");
    assert!(
        new_bytes <= 32 * 1024,
        "one-byte edit re-stored {new_bytes} bytes across {} packs; locality broken",
        new.len()
    );
}

#[test]
fn hardlinks_warn_and_restore_as_independent_copies() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    write_file(&src.join("original"), b"shared bytes");
    std::fs::hard_link(src.join("original"), src.join("alias")).expect("hard_link");

    let mut repo = init_tiny(&root);
    let outcome = repo.backup_path(&src).expect("backup");
    assert!(
        outcome.warnings.iter().any(|w| w.contains("hardlink")),
        "hardlink must be warned about, got: {:?}",
        outcome.warnings
    );

    let dest = td.path().join("restored");
    repo.restore_file(&outcome.snapshot_id, &dest)
        .expect("restore");
    let a = std::fs::metadata(dest.join("original")).expect("meta");
    let b = std::fs::metadata(dest.join("alias")).expect("meta");
    assert_eq!(
        std::fs::read(dest.join("alias")).expect("read"),
        b"shared bytes"
    );
    assert_ne!(
        a.ino(),
        b.ino(),
        "documented divergence: restored as independent copies"
    );
}

#[test]
fn sparse_files_warn_when_the_filesystem_exposes_holes() {
    let td = tempdir().expect("td");
    let src = td.path().join("sparse.bin");
    {
        let mut f = std::fs::File::create(&src).expect("create");
        f.seek(SeekFrom::Start(2 * 1024 * 1024)).expect("seek");
        f.write_all(b"tail").expect("write");
    }
    let meta = std::fs::metadata(&src).expect("meta");
    if meta.blocks() * 512 + 4095 >= meta.len() {
        // Filesystem materialized the hole (no sparse support here); the
        // detection precondition doesn't exist, so there is nothing to test.
        eprintln!("note: fs does not expose sparseness; skipping assertion");
        return;
    }
    let root = td.path().join("repo");
    let mut repo = init_tiny(&root);
    let outcome = repo.backup_path(&src).expect("backup");
    assert!(
        outcome.warnings.iter().any(|w| w.contains("sparse")),
        "sparse file must be warned about, got: {:?}",
        outcome.warnings
    );
}
