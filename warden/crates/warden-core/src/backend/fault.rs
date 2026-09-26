//! Deterministic fault injection: fail exactly the Nth mutating operation.
//! This is how I5 (crash safety) gets tested at *every prefix* of a backup's
//! write sequence, without randomness or real crashes. Reads always pass
//! through untouched — a fault model for "the process died mid-write".

use super::{Backend, Kind};
use crate::error::{Error, Result};
use crate::object::StoredName;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct FaultBackend<B: Backend> {
    inner: B,
    /// Fail when the pre-increment mutation counter equals this.
    fail_at: Option<u64>,
    puts_seen: AtomicU64,
}

impl<B: Backend> FaultBackend<B> {
    pub fn new(inner: B, fail_at: Option<u64>) -> Self {
        FaultBackend {
            inner,
            fail_at,
            puts_seen: AtomicU64::new(0),
        }
    }

    /// Total mutating operations attempted (including the failed one).
    pub fn puts_seen(&self) -> u64 {
        self.puts_seen.load(Ordering::Relaxed)
    }

    fn gate(&self) -> Result<()> {
        let n = self.puts_seen.fetch_add(1, Ordering::Relaxed);
        if Some(n) == self.fail_at {
            return Err(Error::Io(std::io::Error::other("injected fault")));
        }
        Ok(())
    }
}

impl<B: Backend> Backend for FaultBackend<B> {
    fn put(&self, kind: Kind, name: &StoredName, data: &[u8]) -> Result<()> {
        self.gate()?;
        self.inner.put(kind, name, data)
    }
    fn get(&self, kind: Kind, name: &StoredName) -> Result<Vec<u8>> {
        self.inner.get(kind, name)
    }
    fn get_range(&self, kind: Kind, name: &StoredName, offset: u64, len: u64) -> Result<Vec<u8>> {
        self.inner.get_range(kind, name, offset, len) // reads never fault
    }
    fn exists(&self, kind: Kind, name: &StoredName) -> Result<bool> {
        self.inner.exists(kind, name)
    }
    fn list(&self, kind: Kind) -> Result<Vec<StoredName>> {
        self.inner.list(kind)
    }
    fn put_meta(&self, rel_path: &str, data: &[u8], overwrite: bool) -> Result<()> {
        self.gate()?;
        self.inner.put_meta(rel_path, data, overwrite)
    }
    fn get_meta(&self, rel_path: &str) -> Result<Vec<u8>> {
        self.inner.get_meta(rel_path)
    }
    fn size(&self, kind: Kind, name: &StoredName) -> Result<u64> {
        self.inner.size(kind, name)
    }
    fn delete(&self, kind: Kind, name: &StoredName) -> Result<()> {
        self.gate()?;
        self.inner.delete(kind, name)
    }
    fn delete_meta(&self, rel_path: &str) -> Result<()> {
        self.gate()?;
        self.inner.delete_meta(rel_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LocalBackend;
    use crate::object::Hash256;
    use tempfile::tempdir;

    fn name(b: u8) -> StoredName {
        StoredName(Hash256([b; 32]))
    }

    #[test]
    fn fails_exactly_the_nth_mutation_and_passes_reads() {
        let td = tempdir().unwrap();
        let inner = LocalBackend::create(td.path()).unwrap();
        let be = FaultBackend::new(inner, Some(1));
        be.put(Kind::Object, &name(1), b"a").unwrap(); // op 0: passes
        assert!(be.put(Kind::Object, &name(2), b"b").is_err()); // op 1: injected
        be.put(Kind::Object, &name(3), b"c").unwrap(); // op 2: passes again
        assert_eq!(be.puts_seen(), 3);
        // Reads never fault, and the failed op wrote nothing.
        assert_eq!(be.get(Kind::Object, &name(1)).unwrap(), b"a");
        assert!(!be.exists(Kind::Object, &name(2)).unwrap());
        assert_eq!(be.list(Kind::Object).unwrap().len(), 2);
    }

    #[test]
    fn no_fail_at_means_transparent() {
        let td = tempdir().unwrap();
        let be = FaultBackend::new(LocalBackend::create(td.path()).unwrap(), None);
        for i in 0..5u8 {
            be.put(Kind::Object, &name(i), &[i]).unwrap();
        }
        assert_eq!(be.puts_seen(), 5);
    }
}
