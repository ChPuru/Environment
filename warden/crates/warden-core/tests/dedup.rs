//! Dedup economics: identical content stores nothing new; a one-byte edit
//! stores a bounded neighborhood (content-defined chunking locality, I2).

mod common;
use common::*;
use tempfile::tempdir;

#[test]
fn identical_content_stores_zero_new_objects() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let mut repo = init_tiny(&root);
    let data = deterministic_bytes(64 * 1024, 42);

    let a = td.path().join("a.bin");
    write_file(&a, &data);
    repo.backup_file(&a).expect("first backup");
    let baseline = count_objects(&root);
    assert!(
        baseline > 10,
        "want many chunks for a meaningful test, got {baseline}"
    );

    let b = td.path().join("b.bin");
    write_file(&b, &data);
    repo.backup_file(&b).expect("second backup");
    assert_eq!(
        count_objects(&root),
        baseline,
        "identical content must dedup to zero new chunks"
    );
}

#[test]
fn one_byte_edit_stores_a_bounded_neighborhood() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let mut repo = init_tiny(&root);
    let data = deterministic_bytes(64 * 1024, 42);

    let a = td.path().join("a.bin");
    write_file(&a, &data);
    repo.backup_file(&a).expect("first backup");
    let baseline = count_objects(&root);

    let mut edited = data.clone();
    edited[32 * 1024] ^= 0xff;
    let c = td.path().join("c.bin");
    write_file(&c, &edited);
    repo.backup_file(&c).expect("edited backup");

    let new = count_objects(&root) - baseline;
    assert!(new >= 1, "an edit must store something");
    assert!(
        new <= 8,
        "1-byte edit stored {new} new chunks (baseline {baseline}); CDC locality broken"
    );
}
