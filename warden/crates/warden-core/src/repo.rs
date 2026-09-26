use crate::backend::{Backend, Kind, LocalBackend};
use crate::chunker::ChunkerParams;
use crate::crypto::{self, KdfParams, Keyfile, Keys, MasterKey};
use crate::error::{Error, Result};
use crate::index::{Loc, MemIndex};
use crate::object::{
    from_cbor, to_cbor, ChunkId, Compression, Hash256, HealthEvent, HealthRecord, IndexEntry,
    IndexRecord, NameBytes, Node, Snapshot, StoredName, Tree, TreeEntry, HEALTH_VERSION,
    INDEX_VERSION, SNAPSHOT_VERSION,
};
use crate::pack::PackWriter;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const ZSTD_LEVEL: i32 = 3;

/// Plaintext repo config. Contains no user data (sizes leak regardless).
#[derive(Serialize, Deserialize, Debug)]
pub struct Config {
    pub version: u32,
    pub repo_id: [u8; 16],
    pub chunker: ChunkerParams,
    pub compression: Compression,
    /// Pack flush threshold in bytes.
    pub pack_target: u32,
}

pub struct InitOptions {
    pub chunker: ChunkerParams,
    pub kdf: KdfParams,
    pub compression: Compression,
    pub pack_target: u32,
}

impl Default for InitOptions {
    fn default() -> Self {
        InitOptions {
            chunker: ChunkerParams::DEFAULT,
            kdf: KdfParams::DEFAULT,
            compression: Compression::Zstd,
            pack_target: 16 * 1024 * 1024,
        }
    }
}

pub struct SnapshotInfo {
    pub id: String,
    pub created_unix: u64,
    pub source_path: String,
    pub total_len: u64,
    pub file_count: u64,
}

/// What a backup produced: the committed snapshot id plus non-fatal findings
/// (hardlinks stored as copies, sparse files stored dense, ...). Warnings are
/// part of the metadata contract: silently degrading fidelity is not allowed.
pub struct BackupOutcome {
    pub snapshot_id: String,
    pub warnings: Vec<String>,
    /// Identities newly written by THIS backup (read-back's sample space;
    /// also a useful surface for callers auditing what a backup added).
    pub new_chunks: Vec<ChunkId>,
    pub new_trees: Vec<ChunkId>,
    /// Rendered findings from post-commit read-back (empty = clean or off).
    pub readback_problems: Vec<String>,
}

/// Post-commit read-back-after-write: how much of what was just written to
/// re-fetch through the full verification ladder.
#[derive(Clone, Copy, Debug)]
pub enum ReadBack {
    None,
    /// Percentage 1..=100 of newly written chunks and trees (at least one
    /// of each when any were written).
    Sample(u8),
    All,
}

pub struct PruneReport {
    pub snapshots_remaining: u64,
    pub packs_deleted: u64,
    pub bytes_reclaimed: u64,
    pub packs_kept: u64,
    pub dry_run: bool,
}

pub struct DrillReport {
    pub snapshot: String,
    pub files: u64,
    pub bytes: u64,
    pub problems: Vec<String>,
}

struct BackupCtx {
    warnings: Vec<String>,
    seen_inodes: HashMap<(u64, u64), PathBuf>,
    new_entries: Vec<IndexEntry>,
    new_chunks: Vec<ChunkId>,
    new_trees: Vec<ChunkId>,
    total_len: u64,
    file_count: u64,
    pack: PackWriter,
}

#[derive(Default)]
struct RestoreCtx {
    total_len: u64,
    file_count: u64,
}

/// An opened repository: backend + unlocked keys + merged index.
pub struct Repo<B: Backend> {
    backend: B,
    keys: Keys,
    config: Config,
    index: MemIndex,
}

impl Repo<LocalBackend> {
    pub fn init(root: &Path, passphrase: &str, opts: InitOptions) -> Result<Repo<LocalBackend>> {
        Repo::init_backend(LocalBackend::create(root)?, passphrase, opts)
    }

    pub fn open(root: &Path, passphrase: &str) -> Result<Repo<LocalBackend>> {
        Self::open_backend(LocalBackend::open(root)?, passphrase)
    }
}

impl<B: Backend> Repo<B> {
    /// Initialize a repository on any backend (the backend's own creation —
    /// local dirs, S3 bucket — is the caller's business).
    pub fn init_backend(backend: B, passphrase: &str, opts: InitOptions) -> Result<Repo<B>> {
        let mut repo_id = [0u8; 16];
        getrandom::getrandom(&mut repo_id).map_err(|_| Error::Crypto("rng failure"))?;
        let config = Config {
            version: 0,
            repo_id,
            chunker: opts.chunker,
            compression: opts.compression,
            pack_target: opts.pack_target,
        };
        backend.put_meta("config", &to_cbor(&config)?, false)?;
        let mk = MasterKey::generate()?;
        let kf = crypto::wrap_master_key(&mk, passphrase, opts.kdf)?;
        backend.put_meta(
            &format!("keys/{}", hex::encode(repo_id)),
            &to_cbor(&kf)?,
            false,
        )?;
        let keys = Keys::derive(&mk);
        Ok(Repo {
            backend,
            keys,
            config,
            index: MemIndex::default(),
        })
    }

    /// Open against any backend: read config, unwrap keys, merge all indexes.
    pub fn open_backend(backend: B, passphrase: &str) -> Result<Repo<B>> {
        let config: Config = from_cbor(&backend.get_meta("config")?)?;
        let kf: Keyfile =
            from_cbor(&backend.get_meta(&format!("keys/{}", hex::encode(config.repo_id)))?)?;
        let mk = crypto::unwrap_master_key(&kf, passphrase)?;
        let keys = Keys::derive(&mk);
        // Index deltas load tolerantly: a rotten delta is skipped, its chunks
        // become unresolvable (restores fail loudly via missing references),
        // and `verify` names the delta explicitly. Hard-failing open here
        // would make the damage-report tool unable to run on damage.
        let mut index = MemIndex::default();
        for name in backend.list(Kind::Index)? {
            let Ok(sealed) = backend.get(Kind::Index, &name) else {
                continue;
            };
            if verify_stored_name(&sealed, &name).is_err() {
                continue;
            }
            let Ok(plain) = crypto::open(&keys.meta_key, crypto::AD_INDEX, &sealed) else {
                continue;
            };
            if let Ok(rec) = from_cbor::<IndexRecord>(&plain) {
                if rec.version == INDEX_VERSION {
                    index.merge_record(rec);
                }
            }
        }
        Ok(Repo {
            backend,
            keys,
            config,
            index,
        })
    }

    /// Back up a file, directory tree, or symlink. Commit protocol: packs
    /// (chunks + trees) -> index delta -> snapshot last (the commit point).
    pub fn backup_path(&mut self, path: &Path) -> Result<BackupOutcome> {
        self.backup_path_with(path, ReadBack::None)
    }

    /// `backup_path` plus post-commit read-back-after-write. The snapshot is
    /// committed either way; read-back findings are a SCREAM, not a rollback.
    pub fn backup_path_with(&mut self, path: &Path, rb: ReadBack) -> Result<BackupOutcome> {
        let mut ctx = BackupCtx {
            warnings: Vec::new(),
            seen_inodes: HashMap::new(),
            new_entries: Vec::new(),
            new_chunks: Vec::new(),
            new_trees: Vec::new(),
            total_len: 0,
            file_count: 0,
            pack: PackWriter::new(self.config.pack_target as usize),
        };
        let meta = fs::symlink_metadata(path)?;
        let root = self.backup_node(path, &meta, &mut ctx)?;
        self.flush_pack(&mut ctx)?;

        if !ctx.new_entries.is_empty() {
            let rec = IndexRecord {
                version: INDEX_VERSION,
                entries: std::mem::take(&mut ctx.new_entries),
            };
            let sealed = crypto::seal(&self.keys.meta_key, crypto::AD_INDEX, &to_cbor(&rec)?)?;
            self.backend
                .put(Kind::Index, &StoredName::of(&sealed), &sealed)?;
        }

        let snap = Snapshot {
            version: SNAPSHOT_VERSION,
            created_unix: now_unix(),
            source_path: path.display().to_string(),
            root,
            total_len: ctx.total_len,
            file_count: ctx.file_count,
        };
        let sealed = crypto::seal(&self.keys.meta_key, crypto::AD_SNAPSHOT, &to_cbor(&snap)?)?;
        let name = StoredName::of(&sealed);
        self.backend.put(Kind::Snapshot, &name, &sealed)?;
        let mut outcome = BackupOutcome {
            snapshot_id: name.0.to_hex(),
            warnings: ctx.warnings,
            new_chunks: ctx.new_chunks,
            new_trees: ctx.new_trees,
            readback_problems: Vec::new(),
        };
        self.run_readback(rb, &mut outcome)?;
        Ok(outcome)
    }

    fn run_readback(&self, rb: ReadBack, outcome: &mut BackupOutcome) -> Result<()> {
        let (sc, st) = match rb {
            ReadBack::None => return Ok(()),
            ReadBack::All => (outcome.new_chunks.clone(), outcome.new_trees.clone()),
            ReadBack::Sample(pct) => {
                let seed = seed_from_hex(&outcome.snapshot_id);
                (
                    sample_ids(&outcome.new_chunks, pct, seed),
                    sample_ids(&outcome.new_trees, pct, seed ^ 0x9e37_79b9_7f4a_7c15),
                )
            }
        };
        let problems = self.read_back(&sc, &st);
        self.record_health(
            HealthEvent::ReadBack {
                sampled_chunks: sc.len() as u64,
                sampled_trees: st.len() as u64,
                of_chunks: outcome.new_chunks.len() as u64,
                of_trees: outcome.new_trees.len() as u64,
            },
            &problems,
        )?;
        outcome.readback_problems = problems;
        Ok(())
    }

    /// Seal and append one health record. Named by hash like everything
    /// else, so the health log is keylessly scrubbable too.
    fn record_health(&self, event: HealthEvent, problems: &[String]) -> Result<()> {
        let rec = HealthRecord {
            version: HEALTH_VERSION,
            time_unix: now_unix(),
            event,
            problems_total: problems.len() as u64,
            problems_sample: problems.iter().take(8).cloned().collect(),
        };
        let sealed = crypto::seal(&self.keys.meta_key, crypto::AD_HEALTH, &to_cbor(&rec)?)?;
        self.backend
            .put(Kind::Health, &StoredName::of(&sealed), &sealed)
    }

    /// Re-fetch the given identities through the FULL verification ladder
    /// (chunks and trees each via their real read path), returning rendered
    /// findings. This is read-back's engine and its test seam.
    pub fn read_back(&self, chunks: &[ChunkId], trees: &[ChunkId]) -> Vec<String> {
        let mut problems = Vec::new();
        for id in chunks {
            if let Err(e) = self.fetch_chunk_verified(id) {
                problems.push(format!("read-back chunk {}: {e}", id.0.to_hex()));
            }
        }
        for id in trees {
            if let Err(e) = self.load_tree(id) {
                problems.push(format!("read-back tree {}: {e}", id.0.to_hex()));
            }
        }
        problems
    }

    /// Verify and append the outcome to the health log.
    pub fn verify_and_record(&self, deep: bool) -> Result<crate::verify::VerifyReport> {
        let rep = self.verify(deep)?;
        let rendered: Vec<String> = rep.problems.iter().map(|p| p.to_string()).collect();
        self.record_health(
            HealthEvent::Verify {
                deep,
                snapshots: rep.snapshots_checked,
                trees: rep.trees_checked,
                chunks: rep.chunks_checked,
                orphans: rep.orphan_objects,
            },
            &rendered,
        )?;
        Ok(rep)
    }

    /// Restore-drill: prove a randomly chosen snapshot's files actually come
    /// back, byte-perfect, into a scratch dir that is then removed. Recorded.
    pub fn drill(&self, seed: u64) -> Result<DrillReport> {
        let snaps = self.snapshots()?;
        if snaps.is_empty() {
            return Err(Error::Format("no snapshots to drill".into()));
        }
        let pick = &snaps[(seed as usize) % snaps.len()];
        let snap = self.load_snapshot(&pick.id)?;
        let mut files: Vec<(String, Node)> = Vec::new();
        let mut problems: Vec<String> = Vec::new();
        self.collect_files(&snap.root, String::new(), &mut files, &mut problems);

        let scratch = std::env::temp_dir().join(format!(
            "warden-drill-{}-{}",
            std::process::id(),
            RESTORE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(&scratch)?;
        let mut restored = 0u64;
        let mut rctx = RestoreCtx::default();
        for (i, fi) in sample_indices(files.len(), 3, seed).into_iter().enumerate() {
            let (path, node) = &files[fi];
            let dest = scratch.join(i.to_string());
            match self.restore_node(node, &dest, &mut rctx) {
                Ok(()) => restored += 1,
                Err(e) => problems.push(format!("drill restore of {path}: {e}")),
            }
        }
        let _ = fs::remove_dir_all(&scratch);
        self.record_health(
            HealthEvent::Drill {
                snapshot: pick.id.clone(),
                files: restored,
                bytes: rctx.total_len,
            },
            &problems,
        )?;
        Ok(DrillReport {
            snapshot: pick.id.clone(),
            files: restored,
            bytes: rctx.total_len,
            problems,
        })
    }

    /// Walk a snapshot's trees (full load_tree ladder per tree), collecting
    /// restorable files. Tree failures become problems; the walk continues.
    fn collect_files(
        &self,
        node: &Node,
        prefix: String,
        out: &mut Vec<(String, Node)>,
        problems: &mut Vec<String>,
    ) {
        match node {
            Node::File { .. } => out.push((prefix, node.clone())),
            Node::Symlink { .. } => {}
            Node::Dir { tree, .. } => match self.load_tree(tree) {
                Err(e) => problems.push(format!("drill tree at {prefix:?}: {e}")),
                Ok(t) => {
                    for e in &t.entries {
                        let name = String::from_utf8_lossy(&e.name.0).into_owned();
                        let p = if prefix.is_empty() {
                            name
                        } else {
                            format!("{prefix}/{name}")
                        };
                        self.collect_files(&e.node, p, out, problems);
                    }
                }
            },
        }
    }

    /// All decodable health records, oldest first (rotten ones are skipped
    /// here and named by `verify`).
    pub fn health_records(&self) -> Result<Vec<crate::object::HealthRecord>> {
        let mut out = Vec::new();
        for name in self.backend.list(Kind::Health)? {
            let Ok(sealed) = self.backend.get(Kind::Health, &name) else {
                continue;
            };
            if verify_stored_name(&sealed, &name).is_err() {
                continue;
            }
            let Ok(plain) = crypto::open(&self.keys.meta_key, crypto::AD_HEALTH, &sealed) else {
                continue;
            };
            if let Ok(rec) = from_cbor::<HealthRecord>(&plain) {
                out.push(rec);
            }
        }
        out.sort_by_key(|a| a.time_unix);
        Ok(out)
    }

    pub fn health_status(&self) -> Result<crate::health::HealthStatus> {
        Ok(crate::health::status_of(&self.health_records()?))
    }

    /// Convenience wrapper kept for single-file callers; discards warnings.
    pub fn backup_file(&mut self, path: &Path) -> Result<String> {
        Ok(self.backup_path(path)?.snapshot_id)
    }

    /// Compress (per config), seal under `ad`, and stage into the backup's
    /// pack. Identities are always over the UNCOMPRESSED plaintext.
    fn stage_blob(
        &mut self,
        ctx: &mut BackupCtx,
        id: ChunkId,
        ad: &[u8],
        plaintext: &[u8],
    ) -> Result<()> {
        let framed = self.encode_payload(plaintext)?;
        let sealed = crypto::seal(&self.keys.data_key, ad, &framed)?;
        ctx.pack.add(id, &sealed, plaintext.len() as u64);
        if ctx.pack.should_flush() {
            self.flush_pack(ctx)?;
        }
        Ok(())
    }

    fn flush_pack(&mut self, ctx: &mut BackupCtx) -> Result<()> {
        if ctx.pack.is_empty() {
            return Ok(());
        }
        let (bytes, blobs) = ctx.pack.take();
        let pack = StoredName::of(&bytes);
        self.backend.put(Kind::Object, &pack, &bytes)?;
        for b in blobs {
            let e = IndexEntry {
                chunk: b.id,
                pack,
                offset: b.offset,
                sealed_len: b.sealed_len,
                len: b.plain_len,
            };
            self.index.insert(e.clone());
            ctx.new_entries.push(e);
        }
        Ok(())
    }

    /// Frame + maybe-compress a pack-staged blob: [tag u8][payload],
    /// tag 0 = raw, 1 = zstd. The tag is written for EVERY blob, even in
    /// Compression::None repos: one decode path, no config-dependent
    /// parsing. Sniffing (Zstd repos): a 4 KiB sample that will not give
    /// up 2% is stored raw without compressing the rest — incompressible
    /// data should not pay zstd's toll twice (see docs/bench-m8.md); and
    /// if full compression fails to win outright, raw wins anyway.
    fn encode_payload(&self, plaintext: &[u8]) -> Result<Vec<u8>> {
        fn raw(p: &[u8]) -> Vec<u8> {
            let mut v = Vec::with_capacity(p.len() + 1);
            v.push(0u8);
            v.extend_from_slice(p);
            v
        }
        match self.config.compression {
            Compression::None => Ok(raw(plaintext)),
            Compression::Zstd => {
                if plaintext.len() < 64 || sample_incompressible(plaintext) {
                    return Ok(raw(plaintext));
                }
                let c = zstd::encode_all(plaintext, ZSTD_LEVEL)
                    .map_err(|e| Error::Format(format!("zstd encode: {e}")))?;
                if c.len() + 1 >= plaintext.len() {
                    Ok(raw(plaintext))
                } else {
                    let mut v = Vec::with_capacity(c.len() + 1);
                    v.push(1u8);
                    v.extend_from_slice(&c);
                    Ok(v)
                }
            }
        }
    }

    /// Decode a framed blob payload. Config-independent by design.
    fn decompress(&self, payload: Vec<u8>) -> Result<Vec<u8>> {
        match payload.split_first() {
            Some((0, rest)) => Ok(rest.to_vec()),
            Some((1, rest)) => {
                zstd::decode_all(rest).map_err(|e| Error::Format(format!("zstd decode: {e}")))
            }
            _ => Err(Error::Format("empty or unknown compression frame".into())),
        }
    }

    fn backup_node(
        &mut self,
        path: &Path,
        meta: &fs::Metadata,
        ctx: &mut BackupCtx,
    ) -> Result<Node> {
        let ft = meta.file_type();
        if ft.is_symlink() {
            let target = fs::read_link(path)?;
            return Ok(Node::Symlink {
                mtime: meta.mtime(),
                target: NameBytes(target.into_os_string().into_vec()),
            });
        }
        if ft.is_file() {
            if meta.nlink() > 1 {
                let key = (meta.dev(), meta.ino());
                if let Some(first) = ctx.seen_inodes.get(&key) {
                    ctx.warnings.push(format!(
                        "hardlink stored as independent copy: {} ~ {}",
                        path.display(),
                        first.display()
                    ));
                } else {
                    ctx.seen_inodes.insert(key, path.to_path_buf());
                }
            }
            if meta.blocks() * 512 + 4095 < meta.len() {
                ctx.warnings.push(format!(
                    "sparse file stored dense (holes will not be recreated): {}",
                    path.display()
                ));
            }
            let f = fs::File::open(path)?;
            let (len, b3, chunks) = self.store_file_chunks(f, ctx)?;
            ctx.total_len += len;
            ctx.file_count += 1;
            return Ok(Node::File {
                mode: meta.mode(),
                mtime: meta.mtime(),
                len,
                b3,
                chunks,
            });
        }
        if ft.is_dir() {
            let mut names: Vec<(Vec<u8>, PathBuf)> = Vec::new();
            for ent in fs::read_dir(path)? {
                let ent = ent?;
                names.push((ent.file_name().into_vec(), ent.path()));
            }
            names.sort(); // deterministic walk; Tree::new re-sorts defensively
            let mut entries = Vec::with_capacity(names.len());
            for (name, p) in names {
                let m = fs::symlink_metadata(&p)?;
                let node = self.backup_node(&p, &m, ctx)?;
                entries.push(TreeEntry {
                    name: NameBytes(name),
                    node,
                });
            }
            let tree = Tree::new(entries);
            let bytes = to_cbor(&tree)?;
            let tid = self.keys.tree_id(&bytes);
            if !self.index.contains(&tid) && !ctx.pack.contains(&tid) {
                let ad = crypto::tree_ad(&tid);
                self.stage_blob(ctx, tid, &ad, &bytes)?;
                ctx.new_trees.push(tid);
            }
            return Ok(Node::Dir {
                mode: meta.mode(),
                mtime: meta.mtime(),
                tree: tid,
            });
        }
        Err(Error::Format(format!(
            "unsupported file type (fifo/socket/device): {}",
            path.display()
        )))
    }

    fn store_file_chunks(
        &mut self,
        f: fs::File,
        ctx: &mut BackupCtx,
    ) -> Result<(u64, Hash256, Vec<ChunkId>)> {
        let mut hasher = blake3::Hasher::new();
        let mut len: u64 = 0;
        let mut chunk_ids = Vec::new();
        for chunk in crate::chunker::chunk_reader(f, self.config.chunker) {
            let chunk = chunk?;
            hasher.update(&chunk);
            len += chunk.len() as u64;
            let id = self.keys.chunk_id(&chunk);
            if !self.index.contains(&id) && !ctx.pack.contains(&id) {
                let ad = crypto::chunk_ad(&id);
                self.stage_blob(ctx, id, &ad, &chunk)?;
                ctx.new_chunks.push(id);
            }
            chunk_ids.push(id);
        }
        Ok((len, Hash256(*hasher.finalize().as_bytes()), chunk_ids))
    }

    /// Restore a snapshot to `dest` (which must not exist). Every file passes
    /// the verification ladder; snapshot totals are cross-checked at the end;
    /// on any failure `dest` is never created.
    pub fn restore_file(&self, snapshot_id: &str, dest: &Path) -> Result<()> {
        if dest.exists() {
            return Err(Error::DestExists(dest.display().to_string()));
        }
        let snap = self.load_snapshot(snapshot_id)?;
        let dir = match dest.parent() {
            Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let tmp = dir.join(format!(
            ".warden-restore-{}-{}",
            std::process::id(),
            RESTORE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));

        let mut rctx = RestoreCtx::default();
        let assembled = self
            .restore_node(&snap.root, &tmp, &mut rctx)
            .and_then(|()| {
                if rctx.file_count != snap.file_count || rctx.total_len != snap.total_len {
                    return Err(Error::Corrupt {
                        name: "snapshot".into(),
                        reason: "restored totals do not match snapshot",
                    });
                }
                Ok(())
            });

        match assembled {
            Ok(()) => match &snap.root {
                Node::File { .. } => {
                    // hard_link is atomic and refuses an existing destination:
                    // even a file racing into `dest` cannot be clobbered.
                    let link = fs::hard_link(&tmp, dest);
                    let _ = fs::remove_file(&tmp);
                    match link {
                        Ok(()) => Ok(()),
                        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                            Err(Error::DestExists(dest.display().to_string()))
                        }
                        Err(e) => Err(e.into()),
                    }
                }
                _ => {
                    // Directories/symlinks: rename. POSIX rename onto an
                    // existing non-empty dir fails loudly; the pre-check above
                    // covers the rest (narrow race documented in threat model).
                    match fs::rename(&tmp, dest) {
                        Ok(()) => Ok(()),
                        Err(e) => {
                            remove_tree_best_effort(&tmp);
                            Err(e.into())
                        }
                    }
                }
            },
            Err(e) => {
                remove_tree_best_effort(&tmp);
                Err(e)
            }
        }
    }

    fn restore_node(&self, node: &Node, dest: &Path, rctx: &mut RestoreCtx) -> Result<()> {
        match node {
            Node::File {
                mode,
                mtime,
                len,
                b3,
                chunks,
            } => {
                self.restore_file_bytes(dest, *len, b3, chunks)?;
                fs::set_permissions(dest, fs::Permissions::from_mode(mode & 0o7777))?;
                set_mtime(dest, *mtime)?;
                rctx.file_count += 1;
                rctx.total_len += len;
                Ok(())
            }
            Node::Dir { mode, mtime, tree } => {
                fs::create_dir(dest)?;
                let t = self.load_tree(tree)?;
                for e in &t.entries {
                    validate_entry_name(&e.name)?;
                    let child = dest.join(OsStr::from_bytes(&e.name.0));
                    self.restore_node(&e.node, &child, rctx)?;
                }
                // Post-children on purpose: writing children would bump the
                // directory mtime, and a read-only dir would block the writes.
                fs::set_permissions(dest, fs::Permissions::from_mode(mode & 0o7777))?;
                set_mtime(dest, *mtime)?;
                Ok(())
            }
            Node::Symlink { target, .. } => {
                std::os::unix::fs::symlink(OsStr::from_bytes(&target.0), dest)?;
                Ok(())
            }
        }
    }

    fn restore_file_bytes(
        &self,
        dest: &Path,
        expect_len: u64,
        expect_b3: &Hash256,
        chunks: &[ChunkId],
    ) -> Result<()> {
        use std::io::Write;
        let mut out = fs::File::create(dest)?;
        let mut hasher = blake3::Hasher::new();
        let mut written: u64 = 0;
        // Coalesced reads: adjacent chunks in the same pack come back in
        // ONE ranged fetch (round-trips are the enemy on S3), split by
        // sealed_len, then verified INDIVIDUALLY — the per-chunk ladder is
        // unchanged; only the IO shape is. A repeated (dedup) chunk breaks
        // the run naturally: its offset is not where the run's cursor is.
        let mut i = 0;
        while i < chunks.len() {
            let first = *self.index.get(&chunks[i]).ok_or_else(|| Error::Corrupt {
                name: chunks[i].0.to_hex(),
                reason: "chunk missing from index",
            })?;
            let mut run: Vec<(ChunkId, Loc)> = vec![(chunks[i], first)];
            let mut end = first.offset + first.sealed_len;
            let mut j = i + 1;
            while j < chunks.len() {
                match self.index.get(&chunks[j]) {
                    Some(l) if l.pack == first.pack && l.offset == end => {
                        end = l.offset + l.sealed_len;
                        run.push((chunks[j], *l));
                        j += 1;
                    }
                    _ => break, // gap, other pack, or missing (missing errors next round)
                }
            }
            let bytes = self.backend.get_range(
                Kind::Object,
                &first.pack,
                first.offset,
                end - first.offset,
            )?;
            let mut cursor = 0usize;
            for (id, loc) in &run {
                let sealed = bytes[cursor..cursor + loc.sealed_len as usize].to_vec();
                cursor += loc.sealed_len as usize;
                let plain = self.verify_sealed_chunk(id, loc, sealed)?;
                hasher.update(&plain);
                written += plain.len() as u64;
                out.write_all(&plain)?;
            }
            i = j;
        }
        out.sync_all()?;
        if written != expect_len {
            return Err(Error::Corrupt {
                name: "file".into(),
                reason: "length mismatch",
            });
        }
        if Hash256(*hasher.finalize().as_bytes()) != *expect_b3 {
            return Err(Error::Corrupt {
                name: "file".into(),
                reason: "whole-file hash mismatch", // end-to-end check
            });
        }
        Ok(())
    }

    /// Fetch a chunk with the full verification ladder: index resolution,
    /// ranged read (short = LOUD), AEAD under identity-bound AD (rot within
    /// the fetched range is caught here — per-blob AEAD supersedes the old
    /// whole-object name check; whole-PACK hashes remain covered by keyless
    /// scrub and by deep verify), decompression, plaintext length, keyed
    /// identity. Restore's final check — whole-file hash — is at assembly.
    fn fetch_chunk_verified(&self, id: &ChunkId) -> Result<Vec<u8>> {
        let loc = self.index.get(id).ok_or_else(|| Error::Corrupt {
            name: id.0.to_hex(),
            reason: "chunk missing from index",
        })?;
        let sealed = self
            .backend
            .get_range(Kind::Object, &loc.pack, loc.offset, loc.sealed_len)?;
        self.verify_sealed_chunk(id, loc, sealed)
    }

    /// The per-chunk verification ladder, given already-fetched sealed
    /// bytes: AEAD, frame decode, plaintext length, keyed identity.
    fn verify_sealed_chunk(&self, id: &ChunkId, loc: &Loc, sealed: Vec<u8>) -> Result<Vec<u8>> {
        let payload = crypto::open(&self.keys.data_key, &crypto::chunk_ad(id), &sealed)
            .map_err(|_| corrupt(&loc.pack, "chunk authentication failed"))?;
        let plain = self
            .decompress(payload)
            .map_err(|_| corrupt(&loc.pack, "chunk decompression failed"))?;
        if plain.len() as u64 != loc.len {
            return Err(corrupt(&loc.pack, "chunk length mismatch"));
        }
        if self.keys.chunk_id(&plain) != *id {
            return Err(corrupt(&loc.pack, "chunk identity mismatch"));
        }
        Ok(plain)
    }

    fn load_tree(&self, tid: &ChunkId) -> Result<Tree> {
        let loc = self.index.get(tid).ok_or_else(|| Error::Corrupt {
            name: tid.0.to_hex(),
            reason: "tree missing from index",
        })?;
        let sealed = self
            .backend
            .get_range(Kind::Object, &loc.pack, loc.offset, loc.sealed_len)?;
        let payload = crypto::open(&self.keys.data_key, &crypto::tree_ad(tid), &sealed)
            .map_err(|_| corrupt(&loc.pack, "tree authentication failed"))?;
        let plain = self
            .decompress(payload)
            .map_err(|_| corrupt(&loc.pack, "tree decompression failed"))?;
        if plain.len() as u64 != loc.len {
            return Err(corrupt(&loc.pack, "tree length mismatch"));
        }
        if self.keys.tree_id(&plain) != *tid {
            return Err(corrupt(&loc.pack, "tree identity mismatch"));
        }
        from_cbor(&plain)
    }

    /// Reclaim space: delete packs no remaining snapshot references, then
    /// compact the index to a single delta of live entries. PACK
    /// granularity — no repacking in v0, so a pack with one live chunk
    /// survives whole. Refuses to run if any reachable tree or chunk is
    /// missing from the index (prune a damaged repo and you turn damage
    /// into loss). Records a Prune health receipt unless `dry_run`.
    pub fn prune(&mut self, dry_run: bool) -> Result<PruneReport> {
        let (live_entries, live_packs) = self.collect_live()?;
        // Refusal, part two: every LIVE pack must actually exist. Pruning a
        // repo that has already lost data would only deepen the loss.
        for p in &live_packs {
            if !self.backend.exists(Kind::Object, p)? {
                return Err(Error::Corrupt {
                    name: p.0.to_hex(),
                    reason: "live pack missing; refusing to prune a damaged repo",
                });
            }
        }
        let snapshots_remaining = self.snapshots()?.len() as u64;
        let mut dead: Vec<StoredName> = self
            .backend
            .list(Kind::Object)?
            .into_iter()
            .filter(|p| !live_packs.contains(p))
            .collect();
        dead.sort_by_key(|n| n.0 .0); // deterministic sweep order
        let mut bytes_reclaimed = 0u64;
        for p in &dead {
            bytes_reclaimed += self.backend.size(Kind::Object, p)?;
        }
        let report = PruneReport {
            snapshots_remaining,
            packs_deleted: dead.len() as u64,
            bytes_reclaimed,
            packs_kept: live_packs.len() as u64,
            dry_run,
        };
        if dry_run {
            return Ok(report);
        }

        // The order the plan doc argues for: every cut point stays green.
        // 1) One fresh delta holding exactly the live entries.
        let rec = IndexRecord {
            version: INDEX_VERSION,
            entries: live_entries.clone(),
        };
        let sealed = crypto::seal(&self.keys.meta_key, crypto::AD_INDEX, &to_cbor(&rec)?)?;
        let new_name = StoredName::of(&sealed);
        self.backend.put(Kind::Index, &new_name, &sealed)?;
        // 2) Old deltas out (a cut here leaves a live-superset of entries).
        for n in self.backend.list(Kind::Index)? {
            if n != new_name {
                self.backend.delete(Kind::Index, &n)?;
            }
        }
        // 3) Dead packs out (nothing in the compacted index points here).
        for p in &dead {
            self.backend.delete(Kind::Object, p)?;
        }

        let mut idx = MemIndex::default();
        for e in live_entries {
            idx.insert(e);
        }
        self.index = idx;
        self.record_health(
            HealthEvent::Prune {
                snapshots_remaining,
                packs_deleted: report.packs_deleted,
                bytes_reclaimed,
            },
            &[],
        )?;
        Ok(report)
    }

    /// Walk every remaining snapshot; return the live index entries and
    /// the set of packs they inhabit. Err on anything unreachable.
    fn collect_live(&self) -> Result<(Vec<IndexEntry>, HashSet<StoredName>)> {
        let mut seen: HashSet<ChunkId> = HashSet::new();
        let mut ids: Vec<ChunkId> = Vec::new();
        for info in self.snapshots()? {
            let snap = self.load_snapshot(&info.id)?;
            self.mark_node(&snap.root, &mut seen, &mut ids)?;
        }
        let mut entries = Vec::with_capacity(ids.len());
        let mut packs = HashSet::new();
        for id in ids {
            let loc = self.index.get(&id).ok_or_else(|| Error::Corrupt {
                name: id.0.to_hex(),
                reason: "reachable blob missing from index; refusing to prune a damaged repo",
            })?;
            packs.insert(loc.pack);
            entries.push(IndexEntry {
                chunk: id,
                pack: loc.pack,
                offset: loc.offset,
                sealed_len: loc.sealed_len,
                len: loc.len,
            });
        }
        Ok((entries, packs))
    }

    fn mark_node(
        &self,
        node: &Node,
        seen: &mut HashSet<ChunkId>,
        out: &mut Vec<ChunkId>,
    ) -> Result<()> {
        match node {
            Node::File { chunks, .. } => {
                for c in chunks {
                    if seen.insert(*c) {
                        out.push(*c);
                    }
                }
            }
            Node::Symlink { .. } => {}
            Node::Dir { tree, .. } => {
                if seen.insert(*tree) {
                    out.push(*tree);
                }
                let t = self.load_tree(tree)?;
                for e in &t.entries {
                    self.mark_node(&e.node, seen, out)?;
                }
            }
        }
        Ok(())
    }

    /// Delete snapshots by id and/or keep only the newest `keep_last`.
    /// Returns the ids actually deleted. Unknown ids error BEFORE anything
    /// is deleted; `keep_last == 0` is refused (it would erase all
    /// history). "Newest" orders by (created_unix, id) — the id tie-break
    /// makes same-second snapshots deterministic.
    pub fn forget(&self, ids: &[String], keep_last: Option<usize>) -> Result<Vec<String>> {
        if ids.is_empty() && keep_last.is_none() {
            return Err(Error::Format(
                "forget needs snapshot ids or a keep policy".into(),
            ));
        }
        if keep_last == Some(0) {
            return Err(Error::Format(
                "--keep-last 0 would delete every snapshot; refusing".into(),
            ));
        }
        let mut all = self.snapshots()?;
        let mut to_delete: Vec<String> = Vec::new();
        for id in ids {
            if !all.iter().any(|s| s.id == *id) {
                return Err(Error::NotFound(format!("snapshot {id}")));
            }
            if !to_delete.contains(id) {
                to_delete.push(id.clone());
            }
        }
        if let Some(n) = keep_last {
            all.sort_by(|a, b| (a.created_unix, &a.id).cmp(&(b.created_unix, &b.id)));
            let cut = all.len().saturating_sub(n);
            for s in &all[..cut] {
                if !to_delete.contains(&s.id) {
                    to_delete.push(s.id.clone());
                }
            }
        }
        for id in &to_delete {
            let h = Hash256::from_hex(id)
                .map_err(|_| Error::Format(format!("bad snapshot id {id}")))?;
            self.backend.delete(Kind::Snapshot, &StoredName(h))?;
        }
        Ok(to_delete)
    }

    /// The newest snapshot as (id, created_unix), ordered by
    /// (created_unix, id) — same tie-break as `forget --keep-last`.
    /// None when the repository holds no snapshots.
    pub fn newest_snapshot(&self) -> Result<Option<(String, u64)>> {
        Ok(self
            .snapshots()?
            .into_iter()
            .max_by(|a, b| (a.created_unix, &a.id).cmp(&(b.created_unix, &b.id)))
            .map(|s| (s.id, s.created_unix)))
    }

    /// Stable repository identity (hex of the 16-byte id minted at init).
    pub fn repo_id_hex(&self) -> String {
        hex::encode(self.config.repo_id)
    }

    /// Claim the single-writer lock on this repo's backend.
    pub fn lock_writer(&self, ttl_secs: u64) -> Result<crate::lock::LockClaim> {
        crate::lock::acquire(&self.backend, ttl_secs, now_unix())
    }

    pub fn unlock_writer(&self, claim: crate::lock::LockClaim) -> Result<()> {
        crate::lock::release(&self.backend, claim)
    }

    /// Consume the repo, returning its backend (used by fault-injection tests).
    pub fn into_backend(self) -> B {
        self.backend
    }

    /// Keyed verification: audit snapshot/index records and walk
    /// snapshot -> tree -> chunk reachability. See `crate::verify`.
    pub fn verify(&self, deep: bool) -> Result<crate::verify::VerifyReport> {
        use crate::verify::{name_matches, Problem, VerifyReport};

        let mut rep = VerifyReport::default();

        // Raw audit of index deltas: open() skips rotten ones silently by
        // design, so the keyed tier is where they get named.
        for name in self.backend.list(Kind::Index)? {
            let hexn = name.0.to_hex();
            match self.backend.get(Kind::Index, &name) {
                Err(_) => rep.problems.push(Problem::Unreadable {
                    kind: Kind::Index,
                    name: hexn,
                }),
                Ok(sealed) => {
                    if !name_matches(&sealed, &name) {
                        rep.problems.push(Problem::NameMismatch {
                            kind: Kind::Index,
                            name: hexn,
                        });
                    } else {
                        match crypto::open(&self.keys.meta_key, crypto::AD_INDEX, &sealed) {
                            Err(_) => rep.problems.push(Problem::AuthFailed {
                                kind: Kind::Index,
                                name: hexn,
                            }),
                            Ok(plain) => match from_cbor::<IndexRecord>(&plain) {
                                Err(_) => rep.problems.push(Problem::Undecodable {
                                    kind: Kind::Index,
                                    name: hexn,
                                }),
                                Ok(rec) => {
                                    if rec.version != INDEX_VERSION {
                                        rep.problems.push(Problem::Undecodable {
                                            kind: Kind::Index,
                                            name: hexn,
                                        });
                                    }
                                }
                            },
                        }
                    }
                }
            }
        }

        // Health records get the same raw audit (they are part of the
        // repo's promises; a rotten receipt is a problem worth naming).
        for name in self.backend.list(Kind::Health)? {
            let hexn = name.0.to_hex();
            match self.backend.get(Kind::Health, &name) {
                Err(_) => rep.problems.push(Problem::Unreadable {
                    kind: Kind::Health,
                    name: hexn,
                }),
                Ok(sealed) => {
                    if !name_matches(&sealed, &name) {
                        rep.problems.push(Problem::NameMismatch {
                            kind: Kind::Health,
                            name: hexn,
                        });
                    } else {
                        match crypto::open(&self.keys.meta_key, crypto::AD_HEALTH, &sealed) {
                            Err(_) => rep.problems.push(Problem::AuthFailed {
                                kind: Kind::Health,
                                name: hexn,
                            }),
                            Ok(plain) => {
                                if from_cbor::<HealthRecord>(&plain).is_err() {
                                    rep.problems.push(Problem::Undecodable {
                                        kind: Kind::Health,
                                        name: hexn,
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }

        let mut reachable: HashSet<StoredName> = HashSet::new();
        let mut seen_trees: HashSet<ChunkId> = HashSet::new();
        let mut seen_chunks: HashSet<ChunkId> = HashSet::new();
        let mut hashed_packs: HashSet<StoredName> = HashSet::new();

        for name in self.backend.list(Kind::Snapshot)? {
            let id = name.0.to_hex();
            match self.load_snapshot(&id) {
                Err(e) => rep.problems.push(Problem::SnapshotBad {
                    id,
                    reason: e.to_string(),
                }),
                Ok(snap) => {
                    rep.snapshots_checked += 1;
                    self.verify_node(
                        &snap.root,
                        &id,
                        deep,
                        &mut reachable,
                        &mut seen_trees,
                        &mut seen_chunks,
                        &mut hashed_packs,
                        &mut rep,
                    );
                }
            }
        }

        for name in self.backend.list(Kind::Object)? {
            if !reachable.contains(&name) {
                rep.orphan_objects += 1;
            }
        }
        Ok(rep)
    }

    /// Deep mode hashes each touched pack once against its name, preserving
    /// "deep verify catches any at-rest rot affecting reachable data".
    fn deep_hash_pack_once(
        &self,
        pack: &StoredName,
        hashed: &mut std::collections::HashSet<StoredName>,
        rep: &mut crate::verify::VerifyReport,
    ) {
        use crate::verify::{name_matches, Problem};
        if !hashed.insert(*pack) {
            return;
        }
        match self.backend.get(Kind::Object, pack) {
            Err(_) => rep.problems.push(Problem::Unreadable {
                kind: Kind::Object,
                name: pack.0.to_hex(),
            }),
            Ok(bytes) => {
                if !name_matches(&bytes, pack) {
                    rep.problems.push(Problem::NameMismatch {
                        kind: Kind::Object,
                        name: pack.0.to_hex(),
                    });
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_node(
        &self,
        node: &Node,
        snap_id: &str,
        deep: bool,
        reachable: &mut std::collections::HashSet<StoredName>,
        seen_trees: &mut std::collections::HashSet<ChunkId>,
        seen_chunks: &mut std::collections::HashSet<ChunkId>,
        hashed_packs: &mut std::collections::HashSet<StoredName>,
        rep: &mut crate::verify::VerifyReport,
    ) {
        use crate::verify::Problem;
        match node {
            Node::Symlink { .. } => {}
            Node::File { chunks, .. } => {
                for id in chunks {
                    let Some(loc) = self.index.get(id) else {
                        rep.problems.push(Problem::MissingChunk {
                            chunk: id.0.to_hex(),
                            snapshot: snap_id.to_string(),
                        });
                        continue;
                    };
                    let loc = *loc;
                    reachable.insert(loc.pack);
                    if !seen_chunks.insert(*id) {
                        continue;
                    }
                    rep.chunks_checked += 1;
                    if !deep {
                        if !matches!(self.backend.exists(Kind::Object, &loc.pack), Ok(true)) {
                            rep.problems.push(Problem::MissingChunk {
                                chunk: id.0.to_hex(),
                                snapshot: snap_id.to_string(),
                            });
                        }
                        continue;
                    }
                    self.deep_hash_pack_once(&loc.pack, hashed_packs, rep);
                    let hexn = loc.pack.0.to_hex();
                    match self.backend.get_range(
                        Kind::Object,
                        &loc.pack,
                        loc.offset,
                        loc.sealed_len,
                    ) {
                        Err(Error::NotFound(_)) => rep.problems.push(Problem::MissingChunk {
                            chunk: id.0.to_hex(),
                            snapshot: snap_id.to_string(),
                        }),
                        Err(_) => rep.problems.push(Problem::Unreadable {
                            kind: Kind::Object,
                            name: hexn,
                        }),
                        Ok(sealed) => {
                            match crypto::open(&self.keys.data_key, &crypto::chunk_ad(id), &sealed)
                            {
                                Err(_) => rep.problems.push(Problem::AuthFailed {
                                    kind: Kind::Object,
                                    name: hexn,
                                }),
                                Ok(payload) => match self.decompress(payload) {
                                    Err(_) => rep.problems.push(Problem::Undecodable {
                                        kind: Kind::Object,
                                        name: hexn,
                                    }),
                                    Ok(plain) => {
                                        if plain.len() as u64 != loc.len
                                            || self.keys.chunk_id(&plain) != *id
                                        {
                                            rep.problems
                                                .push(Problem::IdentityMismatch { name: hexn });
                                        }
                                    }
                                },
                            }
                        }
                    }
                }
            }
            Node::Dir { tree, .. } => {
                let Some(loc) = self.index.get(tree) else {
                    rep.problems.push(Problem::MissingTree {
                        tree: tree.0.to_hex(),
                        snapshot: snap_id.to_string(),
                    });
                    return;
                };
                let loc = *loc;
                reachable.insert(loc.pack);
                if !seen_trees.insert(*tree) {
                    return; // already fully verified via another snapshot
                }
                rep.trees_checked += 1;
                if deep {
                    self.deep_hash_pack_once(&loc.pack, hashed_packs, rep);
                }
                let hexn = loc.pack.0.to_hex();
                let sealed = match self.backend.get_range(
                    Kind::Object,
                    &loc.pack,
                    loc.offset,
                    loc.sealed_len,
                ) {
                    Err(Error::NotFound(_)) => {
                        rep.problems.push(Problem::MissingTree {
                            tree: tree.0.to_hex(),
                            snapshot: snap_id.to_string(),
                        });
                        return;
                    }
                    Err(_) => {
                        rep.problems.push(Problem::Unreadable {
                            kind: Kind::Object,
                            name: hexn,
                        });
                        return;
                    }
                    Ok(s) => s,
                };
                let payload =
                    match crypto::open(&self.keys.data_key, &crypto::tree_ad(tree), &sealed) {
                        Err(_) => {
                            rep.problems.push(Problem::AuthFailed {
                                kind: Kind::Object,
                                name: hexn,
                            });
                            return;
                        }
                        Ok(p) => p,
                    };
                let plain = match self.decompress(payload) {
                    Err(_) => {
                        rep.problems.push(Problem::Undecodable {
                            kind: Kind::Object,
                            name: hexn,
                        });
                        return;
                    }
                    Ok(p) => p,
                };
                if plain.len() as u64 != loc.len || self.keys.tree_id(&plain) != *tree {
                    rep.problems.push(Problem::IdentityMismatch { name: hexn });
                    return;
                }
                let t: Tree = match from_cbor(&plain) {
                    Err(_) => {
                        rep.problems.push(Problem::Undecodable {
                            kind: Kind::Object,
                            name: hexn,
                        });
                        return;
                    }
                    Ok(t) => t,
                };
                for e in &t.entries {
                    self.verify_node(
                        &e.node,
                        snap_id,
                        deep,
                        reachable,
                        seen_trees,
                        seen_chunks,
                        hashed_packs,
                        rep,
                    );
                }
            }
        }
    }

    pub fn snapshots(&self) -> Result<Vec<SnapshotInfo>> {
        let mut out = Vec::new();
        for name in self.backend.list(Kind::Snapshot)? {
            let snap = self.load_snapshot(&name.0.to_hex())?;
            out.push(SnapshotInfo {
                id: name.0.to_hex(),
                created_unix: snap.created_unix,
                source_path: snap.source_path,
                total_len: snap.total_len,
                file_count: snap.file_count,
            });
        }
        out.sort_by(|a, b| {
            a.created_unix
                .cmp(&b.created_unix)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(out)
    }

    fn load_snapshot(&self, id: &str) -> Result<Snapshot> {
        let name = StoredName(Hash256::from_hex(id)?);
        let sealed = self.backend.get(Kind::Snapshot, &name)?;
        verify_stored_name(&sealed, &name)?;
        let plain = crypto::open(&self.keys.meta_key, crypto::AD_SNAPSHOT, &sealed)
            .map_err(|_| corrupt(&name, "snapshot authentication failed"))?;
        from_cbor(&plain)
    }
}

/// Path-traversal defense in depth: trees are authenticated, but a name with
/// a separator, "..", or NUL must never turn into a path component.
fn validate_entry_name(name: &NameBytes) -> Result<()> {
    let n = &name.0;
    let bad = n.is_empty() || n == b"." || n == b".." || n.contains(&b'/') || n.contains(&0u8);
    if bad {
        return Err(Error::Corrupt {
            name: "tree".into(),
            reason: "unsafe entry name",
        });
    }
    Ok(())
}

fn set_mtime(path: &Path, mtime: i64) -> Result<()> {
    let t = if mtime >= 0 {
        UNIX_EPOCH + Duration::from_secs(mtime as u64)
    } else {
        UNIX_EPOCH - Duration::from_secs(mtime.unsigned_abs())
    };
    let f = fs::File::options().read(true).open(path)?;
    f.set_times(fs::FileTimes::new().set_modified(t))?;
    Ok(())
}

fn remove_tree_best_effort(p: &Path) {
    match fs::symlink_metadata(p) {
        Ok(m) if m.is_dir() => {
            let _ = fs::remove_dir_all(p);
        }
        Ok(_) => {
            let _ = fs::remove_file(p);
        }
        Err(_) => {}
    }
}

fn verify_stored_name(sealed: &[u8], name: &StoredName) -> Result<()> {
    if StoredName::of(sealed) != *name {
        return Err(corrupt(name, "bytes do not match stored name (bit rot)"));
    }
    Ok(())
}

fn corrupt(name: &StoredName, reason: &'static str) -> Error {
    Error::Corrupt {
        name: name.0.to_hex(),
        reason,
    }
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

static RESTORE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// FNV-1a over the snapshot id hex: deterministic sampling seed.
fn seed_from_hex(hexid: &str) -> u64 {
    hexid.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ b as u64).wrapping_mul(0x0100_0000_01b3)
    })
}

fn lcg_next(s: &mut u64) -> u64 {
    *s = s
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *s
}

/// Partial Fisher-Yates: k distinct indices out of len, deterministic in seed.
fn sample_indices(len: usize, k: usize, mut seed: u64) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..len).collect();
    let k = k.min(len);
    for i in 0..k {
        let j = i + (lcg_next(&mut seed) as usize) % (len - i);
        idx.swap(i, j);
    }
    idx.truncate(k);
    idx
}

fn sample_ids(ids: &[ChunkId], pct: u8, seed: u64) -> Vec<ChunkId> {
    if ids.is_empty() || pct == 0 {
        return Vec::new();
    }
    if pct >= 100 {
        return ids.to_vec();
    }
    let k = (ids.len() * pct as usize).div_ceil(100).max(1);
    sample_indices(ids.len(), k, seed)
        .into_iter()
        .map(|i| ids[i])
        .collect()
}

/// Probe the first 4 KiB with fast zstd: if the sample will not give up
/// even 2%, the whole blob is presumed incompressible and stored raw.
fn sample_incompressible(p: &[u8]) -> bool {
    let n = p.len().min(4096);
    match zstd::encode_all(&p[..n], 1) {
        Ok(c) => c.len() * 100 >= n * 98,
        Err(_) => false, // probe failure: fall through to normal compression
    }
}
