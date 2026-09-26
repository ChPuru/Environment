//! mirrorer: make DST an exact copy of SRC. Change detection is honest —
//! same length is NOT same content; hashes decide. Copies are atomic
//! (tmp + rename). Deletions only happen when asked.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default, PartialEq)]
pub struct Plan {
    pub copy: Vec<PathBuf>,   // relative paths to (re)copy
    pub delete: Vec<PathBuf>, // relative paths present only in dst
}

fn walk(root: &Path) -> Result<BTreeMap<PathBuf, u64>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).with_context(|| format!("read {}", d.display()))? {
            let p = e?.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(root).expect("under root").to_path_buf();
                out.insert(rel, fs::metadata(&p)?.len());
            }
        }
    }
    Ok(out)
}

fn hash(p: &Path) -> Result<blake3::Hash> {
    let mut h = blake3::Hasher::new();
    std::io::copy(&mut fs::File::open(p)?, &mut h)?;
    Ok(h.finalize())
}

pub fn plan(src: &Path, dst: &Path) -> Result<Plan> {
    let s = walk(src)?;
    let d = if dst.exists() {
        walk(dst)?
    } else {
        BTreeMap::new()
    };
    let mut plan = Plan::default();
    for (rel, len) in &s {
        match d.get(rel) {
            None => plan.copy.push(rel.clone()),
            Some(dlen) if dlen != len => plan.copy.push(rel.clone()),
            Some(_) => {
                if hash(&src.join(rel))? != hash(&dst.join(rel))? {
                    plan.copy.push(rel.clone()); // same length, different bytes
                }
            }
        }
    }
    for rel in d.keys() {
        if !s.contains_key(rel) {
            plan.delete.push(rel.clone());
        }
    }
    Ok(plan)
}

/// Apply a plan. Deletions run only when `delete` is true. Returns
/// (files_copied, files_deleted).
pub fn apply(src: &Path, dst: &Path, plan: &Plan, delete: bool) -> Result<(usize, usize)> {
    for rel in &plan.copy {
        let to = dst.join(rel);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = to.with_extension("mirrorer-tmp");
        fs::copy(src.join(rel), &tmp)?;
        fs::rename(&tmp, &to)?; // atomic within the filesystem
    }
    let mut deleted = 0;
    if delete {
        for rel in &plan.delete {
            fs::remove_file(dst.join(rel))?;
            deleted += 1;
        }
    }
    Ok((plan.copy.len(), deleted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn td() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "mirrorer-{}-{:x}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn fresh_mirror_then_steady_state_then_targeted_recopy() {
        let base = td();
        let (src, dst) = (base.join("src"), base.join("dst"));
        fs::create_dir_all(src.join("sub")).unwrap();
        fs::write(src.join("a.txt"), b"alpha").unwrap();
        fs::write(src.join("sub/b.bin"), vec![7u8; 5000]).unwrap();

        let p = plan(&src, &dst).unwrap();
        assert_eq!(p.copy.len(), 2);
        apply(&src, &dst, &p, false).unwrap();
        assert_eq!(fs::read(dst.join("sub/b.bin")).unwrap(), vec![7u8; 5000]);

        // Steady state: nothing to do.
        assert_eq!(plan(&src, &dst).unwrap(), Plan::default());

        // Same-length, different bytes: the hash catches it.
        fs::write(src.join("a.txt"), b"ALPHA").unwrap();
        let p = plan(&src, &dst).unwrap();
        assert_eq!(p.copy, vec![PathBuf::from("a.txt")]);
        apply(&src, &dst, &p, false).unwrap();
        assert_eq!(fs::read(dst.join("a.txt")).unwrap(), b"ALPHA");
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn deletions_are_planned_but_only_applied_on_request() {
        let base = td();
        let (src, dst) = (base.join("src"), base.join("dst"));
        fs::create_dir_all(&src).unwrap();
        fs::create_dir_all(&dst).unwrap();
        fs::write(dst.join("stale.txt"), b"old").unwrap();

        let p = plan(&src, &dst).unwrap();
        assert_eq!(p.delete, vec![PathBuf::from("stale.txt")]);
        let (_, deleted) = apply(&src, &dst, &p, false).unwrap();
        assert_eq!(deleted, 0);
        assert!(dst.join("stale.txt").exists(), "no --delete, no deletion");
        let (_, deleted) = apply(&src, &dst, &p, true).unwrap();
        assert_eq!(deleted, 1);
        assert!(!dst.join("stale.txt").exists());
        let _ = fs::remove_dir_all(base);
    }
}
