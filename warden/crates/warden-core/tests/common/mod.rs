#![allow(dead_code)] // helpers are shared across test binaries; not all use all

use std::path::{Path, PathBuf};
use warden_core::backend::LocalBackend;
use warden_core::chunker::ChunkerParams;
use warden_core::crypto::KdfParams;
use warden_core::object::Compression;
use warden_core::repo::{InitOptions, Repo};

pub const PASS: &str = "correct horse battery staple";

/// Mid-size chunker for multi-megabyte tests: enough chunks to be honest,
/// few enough to be fast on one core.
pub const MEDIUM: ChunkerParams = ChunkerParams {
    min: 64 * 1024,
    avg: 256 * 1024,
    max: 1024 * 1024,
};

/// Tests run with zstd ON and a tiny pack target so multi-pack layouts and
/// the compression path are exercised everywhere by default.
pub fn init_with(root: &Path, chunker: ChunkerParams) -> Repo<LocalBackend> {
    Repo::init(
        root,
        PASS,
        InitOptions {
            chunker,
            kdf: KdfParams::MIN_FOR_TESTS,
            compression: Compression::Zstd,
            pack_target: 4096,
        },
    )
    .expect("init")
}

#[allow(dead_code)]
pub fn init_tiny_with_pack(root: &Path, pack_target: u32) -> Repo<LocalBackend> {
    Repo::init(
        root,
        PASS,
        InitOptions {
            chunker: ChunkerParams::TINY_FOR_TESTS,
            kdf: KdfParams::MIN_FOR_TESTS,
            compression: Compression::Zstd,
            pack_target,
        },
    )
    .expect("init")
}

pub fn init_tiny(root: &Path) -> Repo<LocalBackend> {
    init_with(root, ChunkerParams::TINY_FOR_TESTS)
}

pub fn open_tiny(root: &Path) -> Repo<LocalBackend> {
    Repo::open(root, PASS).expect("open")
}

/// Every regular file under the repo except tmp/ (staging is not stored state).
pub fn repo_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(d) = stack.pop() {
        for ent in std::fs::read_dir(&d).expect("read_dir") {
            let p = ent.expect("entry").path();
            if p.is_dir() {
                let is_top_tmp =
                    p.parent() == Some(root) && p.file_name().map(|n| n == "tmp").unwrap_or(false);
                if !is_top_tmp {
                    stack.push(p);
                }
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

pub fn count_objects(root: &Path) -> usize {
    repo_files(root)
        .into_iter()
        .filter(|p| {
            p.strip_prefix(root)
                .map(|r| r.starts_with("objects"))
                .unwrap_or(false)
        })
        .count()
}

/// (path, size) of every file under objects/ — pack accounting for tests.
pub fn object_files_with_sizes(root: &Path) -> Vec<(PathBuf, u64)> {
    repo_files(root)
        .into_iter()
        .filter(|p| {
            p.strip_prefix(root)
                .map(|r| r.starts_with("objects"))
                .unwrap_or(false)
        })
        .map(|p| {
            let len = std::fs::metadata(&p).expect("meta").len();
            (p, len)
        })
        .collect()
}

pub fn write_file(path: &Path, data: &[u8]) {
    std::fs::write(path, data).expect("write");
}

/// xorshift64* — deterministic pseudo-random content with no extra dev-deps.
pub fn deterministic_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut x = seed.max(1);
    let mut v = Vec::with_capacity(len + 8);
    while v.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        v.extend_from_slice(&x.wrapping_mul(0x2545_F491_4F6C_DD1D).to_le_bytes());
    }
    v.truncate(len);
    v
}

// ---- M2 tree helpers ----

use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

/// Build the fixture tree exercising every M2 case: nesting, empty dir,
/// empty file, larger file, symlink, exec bits, restrictive dir mode,
/// non-UTF-8 filename, explicit mtimes.
pub fn build_fixture(root: &Path) {
    use std::fs;
    fs::create_dir_all(root.join("sub/deeper")).expect("dirs");
    fs::create_dir(root.join("empty-dir")).expect("empty dir");
    write_file(&root.join("top.txt"), b"hello tree");
    write_file(
        &root.join("sub/mid.bin"),
        &deterministic_bytes(40 * 1024, 21),
    );
    write_file(
        &root.join("sub/deeper/leaf"),
        &deterministic_bytes(3 * 1024, 22),
    );
    write_file(&root.join("sub/empty-file"), b"");
    let weird = std::ffi::OsStr::from_bytes(b"caf\xE9-\xFF.dat");
    write_file(&root.join(weird), b"non-utf8 name content");
    std::os::unix::fs::symlink("sub/mid.bin", root.join("link-rel")).expect("symlink");
    fs::set_permissions(root.join("top.txt"), fs::Permissions::from_mode(0o755)).expect("chmod");
    fs::set_permissions(root.join("sub/deeper"), fs::Permissions::from_mode(0o700)).expect("chmod");
    set_mtime_for_test(&root.join("top.txt"), 1_500_000_000);
    set_mtime_for_test(&root.join("sub/deeper"), 1_500_000_001);
    set_mtime_for_test(&root.join("empty-dir"), 1_500_000_002);
}

pub fn set_mtime_for_test(p: &Path, secs: i64) {
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(secs as u64);
    let f = std::fs::File::options()
        .read(true)
        .open(p)
        .expect("open for mtime");
    f.set_times(std::fs::FileTimes::new().set_modified(t))
        .expect("set mtime");
}

/// Deep structural + metadata comparison. Asserts (does not return bool) so
/// failures name the exact divergence.
pub fn assert_trees_equal(expected: &Path, actual: &Path) {
    let em = std::fs::symlink_metadata(expected).expect("expected meta");
    let am = std::fs::symlink_metadata(actual).expect("actual meta");
    let kind = |m: &std::fs::Metadata| {
        if m.file_type().is_symlink() {
            "symlink"
        } else if m.is_dir() {
            "dir"
        } else {
            "file"
        }
    };
    assert_eq!(
        kind(&em),
        kind(&am),
        "kind mismatch at {}",
        actual.display()
    );
    if em.file_type().is_symlink() {
        assert_eq!(
            std::fs::read_link(expected).expect("rl"),
            std::fs::read_link(actual).expect("rl"),
            "symlink target mismatch at {}",
            actual.display()
        );
        return; // no mode/mtime contract for symlinks (documented)
    }
    assert_eq!(
        em.mode() & 0o7777,
        am.mode() & 0o7777,
        "mode mismatch at {}",
        actual.display()
    );
    assert_eq!(
        em.mtime(),
        am.mtime(),
        "mtime mismatch at {}",
        actual.display()
    );
    if em.is_dir() {
        let list = |p: &Path| {
            let mut v: Vec<std::ffi::OsString> = std::fs::read_dir(p)
                .expect("read_dir")
                .map(|e| e.expect("ent").file_name())
                .collect();
            v.sort();
            v
        };
        let (en, an) = (list(expected), list(actual));
        assert_eq!(en, an, "entry names mismatch under {}", actual.display());
        for n in en {
            assert_trees_equal(&expected.join(&n), &actual.join(&n));
        }
    } else {
        assert_eq!(
            std::fs::read(expected).expect("read"),
            std::fs::read(actual).expect("read"),
            "content mismatch at {}",
            actual.display()
        );
    }
}

/// The I3 tamper matrix, reusable across snapshot shapes: corrupt every repo
/// file at three positions plus truncation; restore must fail with no
/// destination, or produce output `verify` accepts as identical.
pub fn tamper_matrix(root: &Path, snap: &str, scratch: &Path, verify: &dyn Fn(&Path)) {
    use std::fs;
    let mut case = 0usize;
    for file in repo_files(root) {
        let original = fs::read(&file).expect("read repo file");
        if original.is_empty() {
            continue;
        }
        let mut mutations: Vec<Vec<u8>> = Vec::new();
        for pos in [0usize, original.len() / 2, original.len() - 1] {
            let mut m = original.clone();
            m[pos] ^= 0xff;
            mutations.push(m);
        }
        mutations.push(original[..original.len() / 2].to_vec());
        mutations.dedup();
        for m in mutations {
            case += 1;
            fs::write(&file, &m).expect("inject corruption");
            let dest = scratch.join(format!("out-{case}"));
            let outcome =
                warden_core::repo::Repo::open(root, PASS).and_then(|r| r.restore_file(snap, &dest));
            match outcome {
                Err(_) => assert!(
                    !dest.exists(),
                    "case {case} ({}): restore failed but left a destination",
                    file.display()
                ),
                Ok(()) => verify(&dest),
            }
            fs::write(&file, &original).expect("heal repo file");
        }
    }
    assert!(case > 20, "tamper matrix suspiciously small: {case} cases");
}
