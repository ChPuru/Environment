//! Verification behavior: a clean repo verifies clean; every kind of damage
//! the tamper matrix can inject is *named* by the right tier; orphans are
//! reported but are not problems.

mod common;
use common::*;
use tempfile::tempdir;
use warden_core::verify::{scrub, Problem};

struct Fixture {
    _td: tempfile::TempDir,
    root: std::path::PathBuf,
    snap: String,
}

fn fixture() -> Fixture {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let src = td.path().join("src");
    std::fs::create_dir(&src).expect("mkdir");
    build_fixture(&src);
    let snap = init_tiny(&root)
        .backup_path(&src)
        .expect("backup")
        .snapshot_id;
    Fixture {
        _td: td,
        root,
        snap,
    }
}

fn first_object_file(root: &std::path::Path) -> std::path::PathBuf {
    repo_files(root)
        .into_iter()
        .find(|p| {
            p.strip_prefix(root)
                .map(|r| r.starts_with("objects"))
                .unwrap_or(false)
        })
        .expect("at least one object")
}

#[test]
fn clean_repo_scrubs_and_verifies_clean() {
    let f = fixture();
    let be = warden_core::backend::LocalBackend::open(&f.root).expect("open backend");
    let s = scrub(&be).expect("scrub");
    assert!(
        s.checked > 10,
        "fixture stores many files, checked {}",
        s.checked
    );
    assert!(
        s.problems.is_empty(),
        "clean repo must scrub clean: {:?}",
        s.problems
    );

    let repo = open_tiny(&f.root);
    for deep in [false, true] {
        let v = repo.verify(deep).expect("verify");
        assert_eq!(v.snapshots_checked, 1);
        assert!(
            v.trees_checked >= 4,
            "fixture has >=4 dirs, got {}",
            v.trees_checked
        );
        assert!(v.chunks_checked > 5, "got {}", v.chunks_checked);
        assert_eq!(v.orphan_objects, 0);
        assert!(v.problems.is_empty(), "deep={deep}: {:?}", v.problems);
    }
    let _ = f.snap;
}

#[test]
fn bit_rot_is_named_by_scrub_and_deep_verify() {
    let f = fixture();
    let victim = first_object_file(&f.root);
    let mut bytes = std::fs::read(&victim).expect("read");
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;
    std::fs::write(&victim, &bytes).expect("corrupt");

    let be = warden_core::backend::LocalBackend::open(&f.root).expect("open backend");
    let s = scrub(&be).expect("scrub");
    let victim_name = victim.file_name().unwrap().to_str().unwrap().to_string();
    assert!(
        s.problems
            .iter()
            .any(|p| matches!(p, Problem::NameMismatch { name, .. } if *name == victim_name)),
        "scrub must name the rotten object, got: {:?}",
        s.problems
    );

    let v = open_tiny(&f.root).verify(true).expect("verify");
    assert!(!v.problems.is_empty(), "deep verify must catch bit rot");
}

#[test]
fn fast_verify_catches_missing_but_deep_catches_rot() {
    let f = fixture();
    let victim = first_object_file(&f.root);

    // Rot: bytes present but wrong. Fast (existence-only) may pass; deep must not.
    let original = std::fs::read(&victim).expect("read");
    let mut bytes = original.clone();
    bytes[0] ^= 0xff;
    std::fs::write(&victim, &bytes).expect("corrupt");
    let deep = open_tiny(&f.root).verify(true).expect("verify");
    assert!(!deep.problems.is_empty(), "deep verify must catch rot");

    // Missing: fast must catch it too.
    std::fs::remove_file(&victim).expect("remove");
    let fast = open_tiny(&f.root).verify(false).expect("verify");
    assert!(
        fast.problems.iter().any(|p| matches!(
            p,
            Problem::MissingChunk { .. } | Problem::MissingTree { .. }
        )),
        "fast verify must catch a missing object, got: {:?}",
        fast.problems
    );
}

#[test]
fn orphans_are_counted_but_not_problems() {
    let f = fixture();
    // Plant a well-formed orphan: bytes stored under their own hash, never
    // referenced by any snapshot. Exactly what a crashed backup leaves.
    let orphan_bytes = b"crash leftovers, sealed-shaped but unreachable";
    let name = warden_core::object::StoredName::of(orphan_bytes);
    let hexn = name.0.to_hex();
    let dir = f.root.join("objects").join(&hexn[..2]);
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::write(dir.join(&hexn), orphan_bytes).expect("plant orphan");

    let be = warden_core::backend::LocalBackend::open(&f.root).expect("open backend");
    let s = scrub(&be).expect("scrub");
    assert!(
        s.problems.is_empty(),
        "a well-formed orphan is not rot: {:?}",
        s.problems
    );

    let v = open_tiny(&f.root).verify(true).expect("verify");
    assert_eq!(v.orphan_objects, 1, "orphan must be counted");
    assert!(
        v.problems.is_empty(),
        "orphan must not be a problem: {:?}",
        v.problems
    );
}

#[test]
fn lost_index_surfaces_as_missing_references() {
    let f = fixture();
    for file in repo_files(&f.root) {
        if file
            .strip_prefix(&f.root)
            .map(|r| r.starts_with("indexes"))
            .unwrap_or(false)
        {
            std::fs::remove_file(&file).expect("remove index");
        }
    }
    let repo = open_tiny(&f.root); // opens fine: indexes are additive deltas
    let v = repo.verify(false).expect("verify");
    assert!(
        v.problems.iter().any(|p| matches!(
            p,
            Problem::MissingChunk { .. } | Problem::MissingTree { .. }
        )),
        "losing the index must surface as missing references, got: {:?}",
        v.problems
    );
    assert!(repo.restore_file(&f.snap, &f.root.join("../nope")).is_err());
}

#[test]
fn representative_tampers_are_reported_by_deep_verify() {
    // The tamper matrix proves restore fails; this proves verify *names* the
    // same injuries, for snapshot, index, and object files alike.
    let f = fixture();
    for prefix in ["snapshots", "indexes", "objects"] {
        let victim = repo_files(&f.root)
            .into_iter()
            .find(|p| {
                p.strip_prefix(&f.root)
                    .map(|r| r.starts_with(prefix))
                    .unwrap_or(false)
            })
            .expect("victim");
        let original = std::fs::read(&victim).expect("read");
        let mut bytes = original.clone();
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0xff;
        std::fs::write(&victim, &bytes).expect("corrupt");

        let v = open_tiny(&f.root)
            .verify(true)
            .expect("verify must not abort");
        assert!(
            !v.problems.is_empty(),
            "tamper in {prefix} must be reported"
        );

        std::fs::write(&victim, &original).expect("heal");
    }
    let v = open_tiny(&f.root).verify(true).expect("verify");
    assert!(v.problems.is_empty(), "healed repo must verify clean");
}
