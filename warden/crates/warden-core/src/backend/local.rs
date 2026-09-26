use super::{Backend, Kind};
use crate::error::{Error, Result};
use crate::object::{Hash256, StoredName};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TMP: AtomicU64 = AtomicU64::new(0);

/// Local-filesystem backend. Atomicity: write into `tmp/` inside the repo
/// (same filesystem), fsync, then rename into place — readers never observe
/// partial objects, and `tmp/` is the only place garbage can accumulate.
pub struct LocalBackend {
    root: PathBuf,
}

impl LocalBackend {
    /// Create the on-disk layout for a new repository.
    pub fn create(root: &Path) -> Result<Self> {
        for d in ["objects", "indexes", "snapshots", "keys", "tmp", "health"] {
            fs::create_dir_all(root.join(d))?;
        }
        Ok(LocalBackend {
            root: root.to_path_buf(),
        })
    }

    /// Open an existing repository directory.
    pub fn open(root: &Path) -> Result<Self> {
        if !root.join("config").is_file() {
            return Err(Error::NotFound(format!(
                "no warden repo at {}",
                root.display()
            )));
        }
        Ok(LocalBackend {
            root: root.to_path_buf(),
        })
    }

    fn obj_path(&self, kind: Kind, name: &StoredName) -> PathBuf {
        let hexn = name.0.to_hex();
        match kind {
            // two-level fanout keeps directories sane at scale
            Kind::Object => self.root.join("objects").join(&hexn[..2]).join(&hexn),
            _ => self.root.join(kind.dir()).join(&hexn),
        }
    }

    fn atomic_write(&self, final_path: &Path, data: &[u8]) -> Result<()> {
        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.root.join("tmp").join(format!(
            "w-{}-{}",
            std::process::id(),
            NEXT_TMP.fetch_add(1, Ordering::Relaxed)
        ));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        fs::rename(&tmp, final_path)?;
        // Durability of the *name*, not just the bytes: without fsyncing the
        // parent directory, a crash can forget the rename while keeping the
        // data — exactly the gap the M3 crash tests exist to close.
        if let Some(parent) = final_path.parent() {
            fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    /// Remove staging leftovers from tmp/. Only safe while no writer is
    /// live — single-user CLI assumption until M6 brings locking.
    pub fn clean_tmp(&self) -> Result<u64> {
        let mut removed = 0u64;
        for ent in fs::read_dir(self.root.join("tmp"))? {
            let p = ent?.path();
            if p.is_file() {
                fs::remove_file(&p)?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

impl Backend for LocalBackend {
    fn put(&self, kind: Kind, name: &StoredName, data: &[u8]) -> Result<()> {
        let p = self.obj_path(kind, name);
        if p.exists() {
            // Content-addressed: same name ⇒ same bytes. Never overwrite.
            return Ok(());
        }
        self.atomic_write(&p, data)
    }

    fn get(&self, kind: Kind, name: &StoredName) -> Result<Vec<u8>> {
        match fs::read(self.obj_path(kind, name)) {
            Ok(v) => Ok(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound(format!(
                "{}/{}",
                kind.dir(),
                name.0.to_hex()
            ))),
            Err(e) => Err(e.into()),
        }
    }

    fn exists(&self, kind: Kind, name: &StoredName) -> Result<bool> {
        Ok(self.obj_path(kind, name).is_file())
    }

    fn list(&self, kind: Kind) -> Result<Vec<StoredName>> {
        let mut out = Vec::new();
        let base = self.root.join(kind.dir());
        if !base.is_dir() {
            return Ok(out);
        }
        let mut stack = vec![base];
        while let Some(d) = stack.pop() {
            for ent in fs::read_dir(&d)? {
                let path = ent?.path();
                if path.is_dir() {
                    stack.push(path);
                } else if let Some(nm) = path.file_name().and_then(|s| s.to_str()) {
                    if let Ok(h) = Hash256::from_hex(nm) {
                        out.push(StoredName(h));
                    }
                }
            }
        }
        Ok(out)
    }

    fn get_range(&self, kind: Kind, name: &StoredName, offset: u64, len: u64) -> Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        if len == 0 {
            return Ok(Vec::new()); // canonical degenerate case, no IO
        }
        let mut f = match fs::File::open(self.obj_path(kind, name)) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(Error::NotFound(format!(
                    "{}/{}",
                    kind.dir(),
                    name.0.to_hex()
                )))
            }
            Err(e) => return Err(e.into()),
        };
        f.seek(SeekFrom::Start(offset))?;
        let mut buf = vec![0u8; usize::try_from(len).map_err(|_| super::range_err(name))?];
        f.read_exact(&mut buf).map_err(|e| {
            if e.kind() == std::io::ErrorKind::UnexpectedEof {
                super::range_err(name) // truncated pack: LOUD, never partial
            } else {
                Error::Io(e)
            }
        })?;
        Ok(buf)
    }

    fn put_meta(&self, rel_path: &str, data: &[u8], overwrite: bool) -> Result<()> {
        let p = self.root.join(rel_path);
        if overwrite {
            return self.atomic_write(&p, data);
        }
        // ATOMIC create-new: check-then-write would race, and the writer
        // lock's whole claim to correctness is this path. tmp (fsynced) +
        // hard_link = complete content AND atomic existence failure.
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = self.root.join("tmp").join(format!(
            "meta-{}-{}",
            std::process::id(),
            NEXT_TMP.fetch_add(1, Ordering::Relaxed)
        ));
        {
            let mut f = fs::File::create(&tmp)?;
            f.write_all(data)?;
            f.sync_all()?;
        }
        let link = fs::hard_link(&tmp, &p);
        let _ = fs::remove_file(&tmp);
        match link {
            Ok(()) => {
                if let Some(parent) = p.parent() {
                    fs::File::open(parent)?.sync_all()?;
                }
                Ok(())
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                Err(Error::AlreadyExists(rel_path.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }

    fn size(&self, kind: Kind, name: &StoredName) -> Result<u64> {
        match fs::metadata(self.obj_path(kind, name)) {
            Ok(m) => Ok(m.len()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(Error::NotFound(format!(
                "{}/{}",
                kind.dir(),
                name.0.to_hex()
            ))),
            Err(e) => Err(e.into()),
        }
    }

    fn delete(&self, kind: Kind, name: &StoredName) -> Result<()> {
        match fs::remove_file(self.obj_path(kind, name)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), // idempotent
            Err(e) => Err(e.into()),
        }
    }

    fn delete_meta(&self, rel_path: &str) -> Result<()> {
        match fs::remove_file(self.root.join(rel_path)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()), // idempotent
            Err(e) => Err(e.into()),
        }
    }

    fn get_meta(&self, rel_path: &str) -> Result<Vec<u8>> {
        match fs::read(self.root.join(rel_path)) {
            Ok(v) => Ok(v),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Err(Error::NotFound(rel_path.to_string()))
            }
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn name(b: u8) -> StoredName {
        StoredName(Hash256([b; 32]))
    }

    #[test]
    fn put_get_roundtrips_across_kinds() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        for kind in [Kind::Object, Kind::Index, Kind::Snapshot] {
            be.put(kind, &name(1), b"payload").unwrap();
            assert!(be.exists(kind, &name(1)).unwrap());
            assert_eq!(be.get(kind, &name(1)).unwrap(), b"payload");
        }
    }

    #[test]
    fn put_is_write_once_first_bytes_survive() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        be.put(Kind::Object, &name(2), b"first").unwrap();
        // Idempotent by name: second put succeeds but must not clobber.
        be.put(Kind::Object, &name(2), b"SECOND").unwrap();
        assert_eq!(be.get(Kind::Object, &name(2)).unwrap(), b"first");
    }

    #[test]
    fn get_missing_is_not_found() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        match be.get(Kind::Object, &name(3)) {
            Err(Error::NotFound(_)) => {}
            other => panic!("expected NotFound, got {other:?}"),
        }
    }

    #[test]
    fn list_returns_exactly_what_was_put() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        be.put(Kind::Object, &name(4), b"a").unwrap();
        be.put(Kind::Object, &name(5), b"b").unwrap();
        be.put(Kind::Snapshot, &name(6), b"c").unwrap();
        let mut objs = be.list(Kind::Object).unwrap();
        objs.sort_by_key(|n| n.0 .0);
        assert_eq!(objs, vec![name(4), name(5)]);
        assert_eq!(be.list(Kind::Snapshot).unwrap(), vec![name(6)]);
        assert!(be.list(Kind::Index).unwrap().is_empty());
    }

    #[test]
    fn tmp_dir_holds_no_litter_after_writes() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        be.put(Kind::Object, &name(7), b"x").unwrap();
        be.put_meta("config", b"cfg", false).unwrap();
        let leftovers: Vec<_> = fs::read_dir(td.path().join("tmp")).unwrap().collect();
        assert!(leftovers.is_empty(), "tmp litter: {leftovers:?}");
    }

    #[test]
    fn get_range_reads_slices_and_is_loud_on_truncation() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        let data: Vec<u8> = (0u8..100).collect();
        be.put(Kind::Object, &name(8), &data).unwrap();
        assert_eq!(
            be.get_range(Kind::Object, &name(8), 10, 20).unwrap(),
            &data[10..30]
        );
        assert_eq!(be.get_range(Kind::Object, &name(8), 0, 100).unwrap(), data);
        match be.get_range(Kind::Object, &name(8), 90, 20) {
            Err(Error::Corrupt { .. }) => {}
            other => panic!("range past EOF must be Corrupt, got {other:?}"),
        }
        match be.get_range(Kind::Object, &name(42), 0, 1) {
            Err(Error::NotFound(_)) => {}
            other => panic!("missing object must be NotFound, got {other:?}"),
        }
    }

    #[test]
    fn clean_tmp_removes_stale_staging_only() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        std::fs::write(td.path().join("tmp/stale-1"), b"x").unwrap();
        std::fs::write(td.path().join("tmp/stale-2"), b"y").unwrap();
        be.put(Kind::Object, &name(9), b"real").unwrap();
        assert_eq!(be.clean_tmp().unwrap(), 2);
        assert_eq!(be.get(Kind::Object, &name(9)).unwrap(), b"real");
        assert_eq!(be.clean_tmp().unwrap(), 0);
    }

    #[test]
    fn put_meta_respects_overwrite_flag() {
        let td = tempdir().unwrap();
        let be = LocalBackend::create(td.path()).unwrap();
        be.put_meta("config", b"one", false).unwrap();
        assert!(be.put_meta("config", b"two", false).is_err());
        assert_eq!(be.get_meta("config").unwrap(), b"one");
        be.put_meta("config", b"three", true).unwrap();
        assert_eq!(be.get_meta("config").unwrap(), b"three");
    }
}
