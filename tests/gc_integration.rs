//! Integration tests for garbage collection.
//!
//! These verify the full GC workflow: create checkpoints with content blobs,
//! delete snapshots, run GC, and confirm that unreferenced objects are removed
//! while referenced objects are preserved.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use varn::core::{CheckpointId, CheckpointMeta};
use varn::filesystem::Scanner;
use varn::snapshot::SnapshotData;
use varn::storage::{Repo, garbage_collect};

/// Helper: create a file with content.
fn write_file(root: &Path, path: &str, content: &[u8]) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, content).unwrap();
}

/// Helper: create a checkpoint and store its content blobs.
fn checkpoint(repo: &Repo, description: &str, created_at: i64) -> SnapshotData {
    let scanner = Scanner::new(&repo.root);
    let scan = scanner.scan().unwrap();

    let meta = CheckpointMeta {
        id: CheckpointId("pending".to_string()),
        description: description.to_string(),
        created_at,
        root: repo.root.clone(),
    };
    let snapshot = SnapshotData::new(meta, scan.entries);
    snapshot
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    snapshot.save(&repo.snapshots_dir()).unwrap();
    snapshot
}

/// Helper: count objects in the store.
fn count_objects(repo: &Repo) -> usize {
    repo.object_store().list_objects().unwrap().len()
}

#[test]
fn gc_deletes_unreferenced_after_snapshot_deletion() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Create two checkpoints with different content.
    write_file(tmp.path(), "a.txt", b"content a");
    let snap1 = checkpoint(&repo, "first", 1000);

    write_file(tmp.path(), "a.txt", b"content b");
    let snap2 = checkpoint(&repo, "second", 2000);

    // Both snapshots exist, both objects should be present.
    assert_eq!(count_objects(&repo), 2);

    // Delete the first snapshot file.
    let snap1_path = repo
        .snapshots_dir()
        .join(format!("{}.json", snap1.meta.id.0));
    fs::remove_file(&snap1_path).unwrap();

    // Run GC — the object only referenced by snap1 should be deleted.
    let result = garbage_collect(&repo, false).unwrap();
    assert_eq!(result.deleted, 1);
    assert_eq!(count_objects(&repo), 1);

    // The remaining object should be the one from snap2.
    let remaining = repo.object_store().list_objects().unwrap();
    let snap2_hash = snap2
        .entries
        .iter()
        .find(|e| e.path == Path::new("a.txt"))
        .unwrap()
        .meta
        .hash
        .as_ref()
        .unwrap()
        .clone();
    assert!(remaining.contains(&snap2_hash));
}

#[test]
fn gc_preserves_all_when_all_snapshots_exist() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    write_file(tmp.path(), "a.txt", b"content a");
    checkpoint(&repo, "first", 1000);

    write_file(tmp.path(), "b.txt", b"content b");
    checkpoint(&repo, "second", 2000);

    let objects_before = count_objects(&repo);
    assert_eq!(objects_before, 2);

    let result = garbage_collect(&repo, false).unwrap();
    assert_eq!(result.deleted, 0);
    assert_eq!(count_objects(&repo), 2);
}

#[test]
fn gc_dry_run_does_not_delete() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    write_file(tmp.path(), "a.txt", b"content a");
    let snap1 = checkpoint(&repo, "first", 1000);

    write_file(tmp.path(), "a.txt", b"content b");
    checkpoint(&repo, "second", 2000);

    assert_eq!(count_objects(&repo), 2);

    // Delete first snapshot.
    let snap1_path = repo
        .snapshots_dir()
        .join(format!("{}.json", snap1.meta.id.0));
    fs::remove_file(&snap1_path).unwrap();

    // Dry run — nothing should be deleted.
    let result = garbage_collect(&repo, true).unwrap();
    assert_eq!(result.deleted, 1);
    assert_eq!(count_objects(&repo), 2);

    // Now actually run GC.
    let result = garbage_collect(&repo, false).unwrap();
    assert_eq!(result.deleted, 1);
    assert_eq!(count_objects(&repo), 1);
}

#[test]
fn gc_with_no_snapshots_deletes_all_objects() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Store objects directly without creating snapshots.
    let store = repo.object_store();
    store.store_content("aaaa1111aaaa", b"a").unwrap();
    store.store_content("bbbb2222bbbb", b"b").unwrap();

    assert_eq!(count_objects(&repo), 2);

    let result = garbage_collect(&repo, false).unwrap();
    assert_eq!(result.deleted, 2);
    assert_eq!(count_objects(&repo), 0);
}

#[test]
fn gc_keeps_shared_objects() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Two files with identical content — one object stored.
    write_file(tmp.path(), "a.txt", b"identical");
    write_file(tmp.path(), "b.txt", b"identical");
    let snap1 = checkpoint(&repo, "first", 1000);

    // Both files reference the same hash.
    let hash = snap1
        .entries
        .iter()
        .find(|e| e.path == Path::new("a.txt"))
        .unwrap()
        .meta
        .hash
        .as_ref()
        .unwrap()
        .clone();

    assert_eq!(count_objects(&repo), 1);

    // Delete the snapshot — the object should be collected.
    let snap1_path = repo
        .snapshots_dir()
        .join(format!("{}.json", snap1.meta.id.0));
    fs::remove_file(&snap1_path).unwrap();

    let result = garbage_collect(&repo, false).unwrap();
    assert_eq!(result.deleted, 1);
    assert!(!repo.object_store().exists(&hash));
}

#[test]
fn gc_preserves_objects_across_multiple_checkpoints() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Create three checkpoints with overlapping content.
    write_file(tmp.path(), "shared.txt", b"shared content");
    write_file(tmp.path(), "unique1.txt", b"unique one");
    checkpoint(&repo, "first", 1000);

    // Remove unique1.txt before the second checkpoint so only the first
    // checkpoint references its content.
    fs::remove_file(tmp.path().join("unique1.txt")).unwrap();
    write_file(tmp.path(), "unique2.txt", b"unique two");
    checkpoint(&repo, "second", 2000);

    // Delete the first checkpoint.
    let snapshots = SnapshotData::list_all(&repo.snapshots_dir()).unwrap();
    let snap1_path = repo
        .snapshots_dir()
        .join(format!("{}.json", snapshots[0].meta.id.0));
    fs::remove_file(&snap1_path).unwrap();

    // GC should only delete objects unique to the first checkpoint.
    let result = garbage_collect(&repo, false).unwrap();

    // "shared content" should still exist (referenced by second checkpoint).
    let shared_hash = varn::filesystem::hash_bytes(b"shared content");
    assert!(
        repo.object_store().exists(&shared_hash),
        "shared object should be preserved"
    );

    // "unique one" should be deleted (only in first checkpoint).
    let unique1_hash = varn::filesystem::hash_bytes(b"unique one");
    assert!(
        !repo.object_store().exists(&unique1_hash),
        "unreferenced object should be deleted"
    );

    assert!(result.deleted >= 1);
}
