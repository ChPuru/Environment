use crate::error::{Error, Result};
use crate::object::StoredName;

/// Content-addressed classes a backend stores. Names are ciphertext hashes;
/// the backend never learns anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Object,
    Index,
    Snapshot,
    Health,
}

impl Kind {
    pub fn dir(&self) -> &'static str {
        match self {
            Kind::Object => "objects",
            Kind::Index => "indexes",
            Kind::Snapshot => "snapshots",
            Kind::Health => "health",
        }
    }
}

/// Storage abstraction. Semantics every implementation must honor (the M6
/// conformance suite grows from these): content-addressed writes are atomic,
/// durable-before-visible, write-once, and idempotent by name.
pub trait Backend {
    fn put(&self, kind: Kind, name: &StoredName, data: &[u8]) -> Result<()>;
    fn get(&self, kind: Kind, name: &StoredName) -> Result<Vec<u8>>;
    fn exists(&self, kind: Kind, name: &StoredName) -> Result<bool>;
    fn list(&self, kind: Kind) -> Result<Vec<StoredName>>;

    /// Fixed-path plumbing (config, keyfiles). `overwrite=false` must fail if
    /// the path already holds data.
    /// Read `len` bytes at `offset` within a stored object. The default
    /// reads the whole object (correctness fallback); backends override with
    /// real ranged IO. Short ranges are LOUD failures, never partial data.
    fn get_range(&self, kind: Kind, name: &StoredName, offset: u64, len: u64) -> Result<Vec<u8>> {
        if len == 0 {
            return Ok(Vec::new()); // canonical degenerate case, no IO
        }
        let full = self.get(kind, name)?;
        let start = usize::try_from(offset).map_err(|_| range_err(name))?;
        let l = usize::try_from(len).map_err(|_| range_err(name))?;
        let end = start.checked_add(l).ok_or_else(|| range_err(name))?;
        if end > full.len() {
            return Err(range_err(name));
        }
        Ok(full[start..end].to_vec())
    }

    /// `overwrite=false` MUST fail with Error::AlreadyExists ATOMICALLY if
    /// the path holds data — the writer lock's correctness rides on this.
    fn put_meta(&self, rel_path: &str, data: &[u8], overwrite: bool) -> Result<()>;
    fn get_meta(&self, rel_path: &str) -> Result<Vec<u8>>;

    /// Stored size in bytes; NotFound if absent. Default fetches the whole
    /// object — backends override with metadata/HEAD.
    fn size(&self, kind: Kind, name: &StoredName) -> Result<u64> {
        Ok(self.get(kind, name)?.len() as u64)
    }

    /// Idempotent: deleting something absent is Ok. (Prune's retry story.)
    fn delete(&self, kind: Kind, name: &StoredName) -> Result<()>;
    /// Idempotent, like `delete`.
    fn delete_meta(&self, rel_path: &str) -> Result<()>;
}

/// Trait-object convenience: lets a CLI hold `Box<dyn Backend>` and hand it
/// to the generic `Repo`. Delegation is explicit so inner overrides (real
/// ranged IO, S3 If-None-Match) are never shadowed by defaults.
impl Backend for Box<dyn Backend> {
    fn put(&self, kind: Kind, name: &StoredName, data: &[u8]) -> Result<()> {
        (**self).put(kind, name, data)
    }
    fn get(&self, kind: Kind, name: &StoredName) -> Result<Vec<u8>> {
        (**self).get(kind, name)
    }
    fn exists(&self, kind: Kind, name: &StoredName) -> Result<bool> {
        (**self).exists(kind, name)
    }
    fn list(&self, kind: Kind) -> Result<Vec<StoredName>> {
        (**self).list(kind)
    }
    fn get_range(&self, kind: Kind, name: &StoredName, offset: u64, len: u64) -> Result<Vec<u8>> {
        (**self).get_range(kind, name, offset, len)
    }
    fn put_meta(&self, rel_path: &str, data: &[u8], overwrite: bool) -> Result<()> {
        (**self).put_meta(rel_path, data, overwrite)
    }
    fn get_meta(&self, rel_path: &str) -> Result<Vec<u8>> {
        (**self).get_meta(rel_path)
    }
    fn size(&self, kind: Kind, name: &StoredName) -> Result<u64> {
        (**self).size(kind, name)
    }
    fn delete(&self, kind: Kind, name: &StoredName) -> Result<()> {
        (**self).delete(kind, name)
    }
    fn delete_meta(&self, rel_path: &str) -> Result<()> {
        (**self).delete_meta(rel_path)
    }
}

pub(crate) fn range_err(name: &StoredName) -> Error {
    Error::Corrupt {
        name: name.0.to_hex(),
        reason: "requested range beyond stored bytes",
    }
}

pub mod conformance;
pub mod fault;
pub mod local;
pub mod s3;
pub use fault::FaultBackend;
pub use local::LocalBackend;
