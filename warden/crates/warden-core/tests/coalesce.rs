//! Read coalescing: a many-chunk file restores in a handful of ranged
//! reads, not one per chunk — with the bytes still perfect.

mod common;
use common::*;
use std::sync::atomic::{AtomicU64, Ordering};
use tempfile::tempdir;
use warden_core::backend::{Backend, Kind, LocalBackend};
use warden_core::error::Result;
use warden_core::object::StoredName;
use warden_core::repo::Repo;

/// Delegating backend that counts ranged reads.
struct Counting {
    inner: LocalBackend,
    ranged: AtomicU64,
}

impl Backend for Counting {
    fn put(&self, k: Kind, n: &StoredName, d: &[u8]) -> Result<()> {
        self.inner.put(k, n, d)
    }
    fn get(&self, k: Kind, n: &StoredName) -> Result<Vec<u8>> {
        self.inner.get(k, n)
    }
    fn exists(&self, k: Kind, n: &StoredName) -> Result<bool> {
        self.inner.exists(k, n)
    }
    fn list(&self, k: Kind) -> Result<Vec<StoredName>> {
        self.inner.list(k)
    }
    fn get_range(&self, k: Kind, n: &StoredName, o: u64, l: u64) -> Result<Vec<u8>> {
        self.ranged.fetch_add(1, Ordering::Relaxed);
        self.inner.get_range(k, n, o, l)
    }
    fn put_meta(&self, r: &str, d: &[u8], ow: bool) -> Result<()> {
        self.inner.put_meta(r, d, ow)
    }
    fn get_meta(&self, r: &str) -> Result<Vec<u8>> {
        self.inner.get_meta(r)
    }
    fn size(&self, k: Kind, n: &StoredName) -> Result<u64> {
        self.inner.size(k, n)
    }
    fn delete(&self, k: Kind, n: &StoredName) -> Result<()> {
        self.inner.delete(k, n)
    }
    fn delete_meta(&self, r: &str) -> Result<()> {
        self.inner.delete_meta(r)
    }
}

#[test]
fn restore_reads_runs_not_chunks() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    // One big-ish file, tiny chunker => MANY chunks, one pack (1 MiB target).
    write_file(&src.join("many.bin"), &deterministic_bytes(96 * 1024, 44));
    let mut repo = init_tiny_with_pack(&root, 1 << 20);
    let out = repo.backup_path(&src).expect("backup");
    let n_chunks = out.new_chunks.len();
    assert!(
        n_chunks >= 8,
        "need a many-chunk file for this to mean anything: {n_chunks}"
    );
    drop(repo);

    let counting = Counting {
        inner: LocalBackend::open(&root).expect("open"),
        ranged: AtomicU64::new(0),
    };
    let repo = Repo::open_backend(counting, PASS).expect("reopen");
    let dest = td.path().join("restored");
    repo.restore_file(&out.snapshot_id, &dest).expect("restore");
    assert_trees_equal(&src, &dest);

    let ranged = repo.into_backend().ranged.load(Ordering::Relaxed);
    // Trees fetch individually (a handful); the FILE's chunks must arrive
    // as coalesced runs. Generous bound: far fewer reads than chunks.
    assert!(
        (ranged as usize) * 4 <= n_chunks,
        "coalescing failed: {ranged} ranged reads for {n_chunks} chunks"
    );
}
