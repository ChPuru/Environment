//! The compression frame: one byte that lets incompressible data skip
//! zstd's toll while compressible data keeps the win — in the same repo.

mod common;
use common::*;
use tempfile::tempdir;
use warden_core::chunker::ChunkerParams;
use warden_core::crypto::KdfParams;
use warden_core::object::Compression;
use warden_core::repo::{InitOptions, Repo};

fn stored_object_bytes(root: &std::path::Path) -> u64 {
    object_files_with_sizes(root)
        .into_iter()
        .map(|(_, s)| s)
        .sum()
}

fn init_zstd(root: &std::path::Path) -> Repo<warden_core::backend::LocalBackend> {
    Repo::init(
        root,
        PASS,
        InitOptions {
            chunker: ChunkerParams::TINY_FOR_TESTS,
            kdf: KdfParams::MIN_FOR_TESTS,
            compression: Compression::Zstd,
            pack_target: 1 << 20,
        },
    )
    .expect("init")
}

/// xorshift junk: byte-level entropy zstd cannot beat.
fn incompressible(len: usize, mut x: u64) -> Vec<u8> {
    let mut v = vec![0u8; len];
    for b in v.iter_mut() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *b = x as u8;
    }
    v
}

#[test]
fn incompressible_data_takes_the_raw_path() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    // PRODUCTION chunker here on purpose: this test measures storage
    // overhead, and tiny test chunks pay ~41 bytes of seal+frame each,
    // which would swamp the signal (learned the hard way).
    let mut repo = Repo::init(
        &root,
        PASS,
        InitOptions {
            chunker: ChunkerParams::DEFAULT,
            kdf: KdfParams::MIN_FOR_TESTS,
            compression: Compression::Zstd,
            pack_target: 1 << 20,
        },
    )
    .expect("init");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    let plain = 256 * 1024;
    write_file(&src.join("junk.bin"), &incompressible(plain, 7));
    let out = repo.backup_path(&src).expect("backup");

    let stored = stored_object_bytes(&root);
    // Raw path: stored ~= plain + per-blob seal/frame overhead. If zstd had
    // run and LOST, encode_payload falls back to raw anyway; what this
    // asserts is that storage never balloons past plain + small overhead.
    assert!(
        stored < (plain as u64) * 104 / 100,
        "incompressible data must not balloon: stored {stored} vs plain {plain}"
    );
    assert!(stored > plain as u64, "seal + frame overhead exists");

    // And it comes back byte-perfect through the frame.
    let dest = td.path().join("restored");
    repo.restore_file(&out.snapshot_id, &dest).expect("restore");
    assert_trees_equal(&src, &dest);
}

#[test]
fn compressible_data_still_wins_big_in_the_same_repo() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let mut repo = init_zstd(&root);
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    // Highly repetitive: zstd should crush this.
    let plain: Vec<u8> = b"warden proves its promises. ".repeat(16 * 1024).to_vec();
    write_file(&src.join("text.log"), &plain);
    write_file(&src.join("junk.bin"), &incompressible(128 * 1024, 9));
    let out = repo.backup_path(&src).expect("backup");

    let stored = stored_object_bytes(&root);
    let total_plain = (plain.len() + 128 * 1024) as u64;
    assert!(
        stored < total_plain / 2,
        "mixed repo must still compress the compressible: {stored} vs {total_plain}"
    );

    let dest = td.path().join("restored");
    repo.restore_file(&out.snapshot_id, &dest).expect("restore");
    assert_trees_equal(&src, &dest);
    let vr = repo.verify(true).expect("verify");
    assert!(vr.problems.is_empty(), "{:?}", vr.problems);
}

#[test]
fn a_bad_frame_is_loud_not_garbage() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let mut repo = init_zstd(&root);
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    write_file(&src.join("f.bin"), &deterministic_bytes(30_000, 3));
    repo.backup_path(&src).expect("backup");

    // Flip a byte in every pack: AEAD (not the frame) is the first wall,
    // and deep verify must name every injury rather than emit garbage.
    for (p, _) in object_files_with_sizes(&root) {
        let mut b = std::fs::read(&p).expect("read");
        let mid = b.len() / 2;
        b[mid] ^= 0xff;
        std::fs::write(&p, &b).expect("write");
    }
    let vr = repo.verify(true).expect("verify runs");
    assert!(!vr.problems.is_empty(), "corruption must surface");
}
