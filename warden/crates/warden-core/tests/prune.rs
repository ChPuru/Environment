//! I6, the last invariant: `forget` + `prune` reclaim space WITHOUT ever
//! endangering what remains — under dedup, under dry-run, under damage, and
//! under a fault injected at every single prune mutation.

mod common;
use common::*;
use tempfile::tempdir;
use warden_core::backend::fault::FaultBackend;
use warden_core::backend::{Backend, Kind, LocalBackend};
use warden_core::object::HealthEvent;
use warden_core::repo::Repo;

/// Two snapshots that SHARE data: snap1 has unique.bin + shared.bin,
/// snap2 drops unique.bin, keeps shared.bin, adds fresh.bin. Forgetting
/// snap1 makes unique.bin's packs dead while shared packs must survive.
fn seeded(root: &std::path::Path, base: &std::path::Path) -> (Repo<LocalBackend>, String, String) {
    let mut repo = init_tiny(root);
    let src = base.join("src");
    std::fs::create_dir(&src).expect("mkdir");
    write_file(&src.join("unique.bin"), &deterministic_bytes(40 * 1024, 71));
    write_file(&src.join("shared.bin"), &deterministic_bytes(24 * 1024, 72));
    let s1 = repo.backup_path(&src).expect("backup1").snapshot_id;
    std::fs::remove_file(src.join("unique.bin")).expect("rm");
    write_file(&src.join("fresh.bin"), &deterministic_bytes(10 * 1024, 73));
    let s2 = repo.backup_path(&src).expect("backup2").snapshot_id;
    (repo, s1, s2)
}

fn object_count(root: &std::path::Path) -> usize {
    object_files_with_sizes(root).len()
}

#[test]
fn prune_reclaims_dead_packs_and_never_touches_shared_data() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let (mut repo, s1, s2) = seeded(&root, td.path());
    let before = object_count(&root);

    repo.forget(&[s1], None).expect("forget");

    // Dry-run: full plan, zero deletions, zero receipts.
    let plan = repo.prune(true).expect("dry-run");
    assert!(plan.dry_run && plan.packs_deleted >= 1 && plan.bytes_reclaimed > 0);
    assert_eq!(object_count(&root), before, "dry-run must not delete");
    assert!(
        repo.health_records().expect("recs").is_empty(),
        "dry-run must not write receipts"
    );

    // Real prune: dead packs gone, exactly what the plan promised.
    let rep = repo.prune(false).expect("prune");
    assert!(!rep.dry_run);
    assert_eq!(rep.packs_deleted, plan.packs_deleted);
    assert_eq!(rep.bytes_reclaimed, plan.bytes_reclaimed);
    assert_eq!(rep.snapshots_remaining, 1);
    assert_eq!(object_count(&root), before - rep.packs_deleted as usize);
    assert!(rep.packs_kept >= 1, "shared/fresh data still needs packs");

    // Index compacted to exactly one delta.
    let be = LocalBackend::open(&root).expect("open");
    assert_eq!(be.list(Kind::Index).expect("list").len(), 1);

    // The survivor is untouched: deep verify green, zero orphans, restore
    // byte-perfect, and the receipt is on file.
    let rep2 = repo.verify(true).expect("verify");
    assert!(rep2.problems.is_empty(), "{:?}", rep2.problems);
    assert_eq!(rep2.orphan_objects, 0, "clean prune leaves no orphans");
    let dest = td.path().join("restored");
    repo.restore_file(&s2, &dest).expect("restore");
    assert_trees_equal(&td.path().join("src"), &dest);
    let recs = repo.health_records().expect("recs");
    assert!(matches!(
        recs.last().expect("rec").event,
        HealthEvent::Prune {
            snapshots_remaining: 1,
            ..
        }
    ));

    // Steady state: a second prune finds nothing to do.
    let rep3 = repo.prune(false).expect("prune again");
    assert_eq!(rep3.packs_deleted, 0);
    assert_eq!(rep3.bytes_reclaimed, 0);
}

#[test]
fn pruning_away_all_history_empties_the_object_store() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let (mut repo, s1, s2) = seeded(&root, td.path());
    repo.forget(&[s1, s2], None).expect("forget all");
    let rep = repo.prune(false).expect("prune");
    assert_eq!(rep.snapshots_remaining, 0);
    assert!(rep.packs_deleted >= 1 && rep.packs_kept == 0);
    assert_eq!(object_count(&root), 0, "no history, no packs");
    let vr = repo.verify(true).expect("verify");
    assert!(vr.problems.is_empty(), "{:?}", vr.problems);
}

#[test]
fn prune_refuses_to_run_on_a_damaged_repo() {
    let td = tempdir().expect("td");
    let root = td.path().join("repo");
    let (mut repo, _s1, _s2) = seeded(&root, td.path());
    // NO forget: every pack is LIVE, so any victim is live data. (A missing
    // DEAD pack must NOT refuse — recovery-by-prune after a partial crash
    // depends on that — and the fault sweep exercises exactly those states.)
    let (victim, _) = object_files_with_sizes(&root)
        .into_iter()
        .next()
        .expect("a pack");
    std::fs::remove_file(victim).expect("rm");
    let before = object_count(&root);
    assert!(
        repo.prune(false).is_err(),
        "pruning damage turns damage into loss"
    );
    assert_eq!(object_count(&root), before, "refusal must delete nothing");
    let be = LocalBackend::open(&root).expect("open");
    assert_eq!(
        be.list(Kind::Index).expect("list").len(),
        2,
        "refusal before any compaction"
    );
}

/// The crash half of I6: fail every single prune mutation in turn; after
/// each cut, a fresh open must deep-verify green and restore the survivor
/// byte-perfect. (The deletion ORDER in the plan doc is what this proves.)
#[test]
fn prune_interrupted_at_every_mutation_leaves_repo_green() {
    let td = tempdir().expect("td");

    // One flow, used for baseline and every iteration, so mutation counts
    // are shape-identical by construction.
    let prepare = |tag: &str| -> (std::path::PathBuf, std::path::PathBuf, String) {
        let root = td.path().join(format!("r{tag}"));
        let dir = td.path().join(format!("d{tag}"));
        std::fs::create_dir(&dir).expect("mkdir");
        let (repo, s1, s2) = seeded(&root, &dir);
        repo.forget(&[s1], None).expect("forget");
        drop(repo);
        (root, dir, s2)
    };

    // Baseline twin: count prune's mutations unfaulted.
    let (root, _dir, _s2) = prepare("base");
    let fb = FaultBackend::new(LocalBackend::open(&root).expect("open"), None);
    let mut frepo = Repo::open_backend(fb, PASS).expect("open fault");
    frepo.prune(false).expect("baseline prune");
    let total = frepo.into_backend().puts_seen();
    assert!(
        total >= 3,
        "expected at least index-put + deletes, got {total}"
    );

    // gate() is zero-indexed: fail_at == n fails the (n+1)th mutation.
    for fail_at in 0..total {
        let (root, dir, s2) = prepare(&fail_at.to_string());
        let fb = FaultBackend::new(LocalBackend::open(&root).expect("open"), Some(fail_at));
        let mut frepo = Repo::open_backend(fb, PASS).expect("open fault");
        assert!(
            frepo.prune(false).is_err(),
            "fault at {fail_at} must surface"
        );
        drop(frepo);

        // Every cut point must leave a green, restorable repo.
        let repo = Repo::open(&root, PASS).expect("reopen after cut");
        let vr = repo.verify(true).expect("verify");
        assert!(
            vr.problems.is_empty(),
            "cut at mutation {fail_at} broke the repo: {:?}",
            vr.problems
        );
        let dest = dir.join("restored");
        repo.restore_file(&s2, &dest).expect("restore after cut");
        assert_trees_equal(&dir.join("src"), &dest);
    }
}
