//! GC contract regressions (field report: orphan detection and referenced-
//! object preservation verified working — pinned).

use crate::common::TestRepo;
use std::fs;

#[test]
fn gc_keeps_everything_when_all_referenced() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");
    repo.checkpoint("keep");

    let result = varn::storage::garbage_collect(&repo.repo, false).unwrap();
    assert_eq!(result.deleted, 0);
    assert_eq!(result.total_objects, 2);
    assert_eq!(result.referenced_objects, 2);
}

#[test]
fn gc_collects_orphaned_object() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"orphan me");
    let snapshot = repo.checkpoint("then delete snapshot");

    let snap_path = repo
        .repo
        .snapshots_dir()
        .join(format!("{}.json", snapshot.meta.id.0));
    fs::remove_file(&snap_path).unwrap();

    let result = varn::storage::garbage_collect(&repo.repo, false).unwrap();
    assert_eq!(result.deleted, 1);
    assert_eq!(result.total_objects, 1);
    assert_eq!(result.referenced_objects, 0);
    assert_eq!(result.deleted_hashes.len(), 1);
}

#[test]
fn gc_dry_run_reports_without_deleting() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"data");
    let snapshot = repo.checkpoint("then delete");
    let snap_path = repo
        .repo
        .snapshots_dir()
        .join(format!("{}.json", snapshot.meta.id.0));
    fs::remove_file(&snap_path).unwrap();

    let result = varn::storage::garbage_collect(&repo.repo, true).unwrap();
    assert_eq!(result.deleted, 1, "dry run reports the orphan count");
    assert_eq!(result.deleted_hashes.len(), 1, "and lists the hash");
    let hash = &result.deleted_hashes[0];
    assert!(
        repo.repo.object_store().exists(hash),
        "object must still exist after dry run"
    );
}

#[test]
fn gc_shared_object_survives_one_snapshot_delete() {
    // Two checkpoints share an object (identical content). Deleting ONE
    // snapshot must not collect the object the other still references.
    let repo = TestRepo::new();
    repo.write("a.txt", b"shared");
    let _cp1 = repo.checkpoint("one");
    repo.write("b.txt", b"shared"); // same content, different path
    let cp3 = repo.checkpoint("three");

    // Both snapshots reference the SAME object (identical content).
    let shared_hash = varn::filesystem::hash_bytes(b"shared");
    // Delete cp3's snapshot: cp1 still references the shared object.
    fs::remove_file(
        repo.repo
            .snapshots_dir()
            .join(format!("{}.json", cp3.meta.id.0)),
    )
    .unwrap();

    let result = varn::storage::garbage_collect(&repo.repo, false).unwrap();
    assert!(
        repo.repo.object_store().exists(&shared_hash),
        "shared object must survive while cp1 references it"
    );
    let _ = result;
}

#[test]
fn gc_after_restore_keeps_everything() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    let snapshot = repo.checkpoint("keep");
    repo.write("a.txt", b"changed");
    repo.restore(&snapshot);

    let result = varn::storage::garbage_collect(&repo.repo, false).unwrap();
    assert_eq!(result.deleted, 0);
    assert!(repo.verifies(&snapshot));
}
