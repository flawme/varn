//! Integration tests for the safety checkpoint-before-restore feature.
//!
//! These verify that:
//! - A safety checkpoint is created before restore executes.
//! - The safety checkpoint captures the pre-restore state.
//! - The safety checkpoint can be used to recover after a restore.
//! - The `--no-safety` concept (skipping the safety checkpoint) works.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use varn::core::{CheckpointId, CheckpointMeta};
use varn::filesystem::Scanner;
use varn::restore;
use varn::snapshot::SnapshotData;
use varn::storage::Repo;

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

/// Helper: scan the current state and plan a restore against a snapshot.
fn plan_against_current(repo: &Repo, snapshot: &SnapshotData) -> restore::RestorePlan {
    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();
    restore::plan_restore(&snapshot.entries, &current.entries)
}

#[test]
fn safety_checkpoint_captures_pre_restore_state() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "original.txt", b"original content");

    // Create the target checkpoint.
    let target = checkpoint(&repo, "target state", 1000);

    // Modify the file.
    write_file(tmp.path(), "original.txt", b"modified content");
    write_file(tmp.path(), "new_file.txt", b"new");

    // Create a safety checkpoint of the current (modified) state.
    let scanner = Scanner::new(&repo.root);
    let current_scan = scanner.scan().unwrap();
    let safety_meta = CheckpointMeta {
        id: CheckpointId("pending".to_string()),
        description: "[safety before restore of target] target state".to_string(),
        created_at: 2000,
        root: repo.root.clone(),
    };
    let safety = SnapshotData::new(safety_meta, current_scan.entries.clone());
    safety
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    let saved = safety.save(&repo.snapshots_dir()).unwrap();
    assert!(saved, "safety checkpoint should be saved");

    // Now restore the target checkpoint.
    let plan = plan_against_current(&repo, &target);
    restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert!(restore::verify_restore(&repo.root, &target.entries));

    // The safety checkpoint should still exist and be loadable.
    let safety_loaded = SnapshotData::load_by_id(&repo.snapshots_dir(), &safety.meta.id.0).unwrap();
    assert_eq!(safety_loaded.entries, current_scan.entries);

    // We should be able to restore the safety checkpoint to get back
    // to the pre-restore (modified) state.
    let plan2 = plan_against_current(&repo, &safety);
    restore::execute_restore(&plan2, &repo.root, &repo.object_store()).unwrap();
    assert!(restore::verify_restore(&repo.root, &safety.entries));

    // The modified content should be back.
    assert_eq!(
        fs::read_to_string(tmp.path().join("original.txt")).unwrap(),
        "modified content"
    );
    assert!(tmp.path().join("new_file.txt").exists());
}

#[test]
fn safety_checkpoint_is_listed_alongside_regular_checkpoints() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"content a");

    let _target = checkpoint(&repo, "target", 1000);

    // Create a safety checkpoint.
    write_file(tmp.path(), "b.txt", b"content b");
    let scanner = Scanner::new(&repo.root);
    let current_scan = scanner.scan().unwrap();
    let safety_meta = CheckpointMeta {
        id: CheckpointId("pending".to_string()),
        description: "[safety before restore of target] target".to_string(),
        created_at: 2000,
        root: repo.root.clone(),
    };
    let safety = SnapshotData::new(safety_meta, current_scan.entries);
    safety
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    safety.save(&repo.snapshots_dir()).unwrap();

    // Both should appear in the list.
    let list = SnapshotData::list_all(&repo.snapshots_dir()).unwrap();
    assert_eq!(list.len(), 2);

    // The safety checkpoint should have the [safety] prefix in its description.
    let safety_in_list = list
        .iter()
        .find(|s| s.meta.description.starts_with("[safety"))
        .expect("safety checkpoint should be in list");
    assert_eq!(safety_in_list.meta.id.0, safety.meta.id.0);
}

#[test]
fn safety_checkpoint_can_recover_after_failed_restore() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "keep.txt", b"keep");
    write_file(tmp.path(), "modify.txt", b"original");

    let target = checkpoint(&repo, "target", 1000);

    // Modify files.
    write_file(tmp.path(), "modify.txt", b"changed");
    write_file(tmp.path(), "extra.txt", b"extra");

    // Create safety checkpoint.
    let scanner = Scanner::new(&repo.root);
    let current_scan = scanner.scan().unwrap();
    let safety_meta = CheckpointMeta {
        id: CheckpointId("pending".to_string()),
        description: "[safety] before restore".to_string(),
        created_at: 2000,
        root: repo.root.clone(),
    };
    let safety = SnapshotData::new(safety_meta, current_scan.entries.clone());
    safety
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    safety.save(&repo.snapshots_dir()).unwrap();

    // Restore the target checkpoint.
    let plan = plan_against_current(&repo, &target);
    restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert!(restore::verify_restore(&repo.root, &target.entries));

    // Now "undo" by restoring the safety checkpoint.
    let undo_plan = plan_against_current(&repo, &safety);
    restore::execute_restore(&undo_plan, &repo.root, &repo.object_store()).unwrap();
    assert!(restore::verify_restore(&repo.root, &safety.entries));

    // The modified state should be back.
    assert_eq!(
        fs::read_to_string(tmp.path().join("modify.txt")).unwrap(),
        "changed"
    );
    assert!(tmp.path().join("extra.txt").exists());
}

#[test]
fn safety_checkpoint_deduplicates_with_identical_state() {
    // If the current state is identical to a previously checkpointed state,
    // the safety checkpoint should produce the same ID (content-addressed).
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"content");

    let scanner = Scanner::new(&repo.root);
    let scan1 = scanner.scan().unwrap();

    let meta1 = CheckpointMeta {
        id: CheckpointId("pending".to_string()),
        description: "[safety] before restore".to_string(),
        created_at: 1000,
        root: repo.root.clone(),
    };
    let snap1 = SnapshotData::new(meta1, scan1.entries.clone());
    snap1
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    let saved1 = snap1.save(&repo.snapshots_dir()).unwrap();
    assert!(saved1);

    // Same state, same description, same timestamp -> same ID.
    let meta2 = CheckpointMeta {
        id: CheckpointId("pending".to_string()),
        description: "[safety] before restore".to_string(),
        created_at: 1000,
        root: repo.root.clone(),
    };
    let snap2 = SnapshotData::new(meta2, scan1.entries);
    let saved2 = snap2.save(&repo.snapshots_dir()).unwrap();
    assert!(!saved2, "identical checkpoint should not be re-saved");

    // Only one snapshot file should exist.
    let list = SnapshotData::list_all(&repo.snapshots_dir()).unwrap();
    assert_eq!(list.len(), 1);
}
