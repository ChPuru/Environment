//! I7: the backend never receives plaintext content or names. Markers are
//! planted in file bytes, a filename, and a directory name; after a tree
//! backup, no stored file's bytes and no stored path may contain them.

mod common;
use common::*;
use std::os::unix::ffi::OsStrExt;
use tempfile::tempdir;

const CANARY: &[u8] = b"WARDEN-CANARY-9f31c2d7a5e8";

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn plaintext_reaches_storage_in_neither_content_nor_names() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let marker = std::str::from_utf8(CANARY).expect("ascii");

    let src = td.path().join("src");
    let secret_dir = src.join(format!("dir-{marker}"));
    std::fs::create_dir_all(&secret_dir).expect("mkdir");
    let mut data = deterministic_bytes(8 * 1024, 5);
    let mid = data.len() / 2;
    data.splice(mid..mid, CANARY.iter().copied());
    write_file(&secret_dir.join(format!("file-{marker}.txt")), &data);
    std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(CANARY), src.join("sneaky-link"))
        .expect("symlink");

    init_tiny(&root).backup_path(&src).expect("backup");

    for f in repo_files(&root) {
        let bytes = std::fs::read(&f).expect("read repo file");
        assert!(
            !contains(&bytes, CANARY),
            "canary in stored bytes: {}",
            f.display()
        );
        let rel = f
            .strip_prefix(&root)
            .expect("under root")
            .display()
            .to_string();
        assert!(
            !contains(rel.as_bytes(), CANARY),
            "canary in stored path: {rel}"
        );
    }
}
