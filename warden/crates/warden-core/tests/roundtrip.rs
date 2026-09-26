//! I1: restore(backup(x)) == x, byte for byte, through a full reopen
//! (persistence + key unwrap + index load are all on the tested path).

mod common;
use common::*;
use proptest::prelude::*;
use tempfile::tempdir;
use warden_core::chunker::ChunkerParams;

fn roundtrip_with(data: &[u8], params: ChunkerParams) {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src.bin");
    write_file(&src, data);

    let snap = {
        let mut repo = init_with(&root, params);
        repo.backup_file(&src).expect("backup")
    };

    // Reopen from disk: nothing may survive in memory from the writer.
    let repo = open_tiny(&root);
    let dest = td.path().join("out.bin");
    repo.restore_file(&snap, &dest).expect("restore");
    assert_eq!(std::fs::read(&dest).expect("read"), data, "I1 violated");
}

#[test]
fn empty_file_roundtrips() {
    roundtrip_with(&[], ChunkerParams::TINY_FOR_TESTS);
}

#[test]
fn single_byte_roundtrips() {
    roundtrip_with(&[0x42], ChunkerParams::TINY_FOR_TESTS);
}

#[test]
fn eight_mib_roundtrips() {
    roundtrip_with(&deterministic_bytes(8 * 1024 * 1024, 7), MEDIUM);
}

#[test]
fn restore_refuses_existing_destination() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src.bin");
    write_file(&src, b"data");
    let mut repo = init_tiny(&root);
    let snap = repo.backup_file(&src).expect("backup");
    let dest = td.path().join("out.bin");
    write_file(&dest, b"precious existing bytes");
    assert!(repo.restore_file(&snap, &dest).is_err());
    assert_eq!(
        std::fs::read(&dest).expect("read"),
        b"precious existing bytes"
    );
}

#[test]
fn roundtrip_without_compression() {
    use warden_core::crypto::KdfParams;
    use warden_core::object::Compression;
    use warden_core::repo::{InitOptions, Repo};
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src.bin");
    let data = deterministic_bytes(30_000, 3);
    write_file(&src, &data);
    let snap = Repo::init(
        &root,
        PASS,
        InitOptions {
            chunker: ChunkerParams::TINY_FOR_TESTS,
            kdf: KdfParams::MIN_FOR_TESTS,
            compression: Compression::None,
            pack_target: 4096,
        },
    )
    .expect("init")
    .backup_file(&src)
    .expect("backup");
    let dest = td.path().join("out.bin");
    open_tiny(&root)
        .restore_file(&snap, &dest)
        .expect("restore");
    assert_eq!(std::fs::read(&dest).expect("read"), data);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]
    #[test]
    fn prop_restore_backup_identity(
        data in proptest::collection::vec(any::<u8>(), 0..20_000)
    ) {
        roundtrip_with(&data, ChunkerParams::TINY_FOR_TESTS);
    }
}
