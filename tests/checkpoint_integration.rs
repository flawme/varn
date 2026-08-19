//! Integration tests for the checkpoint, list, and diff workflow.
//!
//! These exercise the full end-to-end workflow: init → checkpoint → list →
//! modify files → diff. They verify snapshot persistence, content-addressed
//! storage deduplication, and change detection.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use varn::core::CheckpointMeta;
use varn::filesystem::{EntryKind, Scanner, hash_bytes};
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

/// Helper: create a checkpoint for the given repo.
fn create_checkpoint(repo: &Repo, description: &str) -> SnapshotData {
    let scanner = Scanner::new(&repo.root);
    let scan_result = scanner.scan().unwrap();

    let meta = CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: description.to_string(),
        created_at: 1_000_000,
        root: repo.root.clone(),
    };
    let snapshot = SnapshotData::new(meta, scan_result.entries);
    snapshot
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    snapshot.save(&repo.snapshots_dir()).unwrap();
    snapshot
}

#[test]
fn checkpoint_creates_snapshot_file() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"hello");

    let snapshot = create_checkpoint(&repo, "first checkpoint");

    // The snapshot file should exist.
    let snapshot_path = repo
        .snapshots_dir()
        .join(format!("{}.json", snapshot.meta.id.0));
    assert!(snapshot_path.exists(), "snapshot file must exist");
}

#[test]
fn checkpoint_stores_content_blobs() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"hello world");

    let snapshot = create_checkpoint(&repo, "test");

    // The content blob should exist in the object store.
    let file_entry = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("a.txt"))
        .unwrap();
    let hash = file_entry.meta.hash.as_ref().unwrap();
    assert!(
        repo.object_store().exists(hash),
        "content blob must be stored"
    );
    let content = repo.object_store().read_content(hash).unwrap();
    assert_eq!(content, b"hello world");
}

#[test]
fn checkpoint_deduplicates_identical_content() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"identical content");
    write_file(tmp.path(), "b.txt", b"identical content");

    let snapshot = create_checkpoint(&repo, "dedup test");

    // Both files should have the same hash.
    let hash_a = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("a.txt"))
        .unwrap()
        .meta
        .hash
        .as_ref()
        .unwrap()
        .clone();
    let hash_b = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("b.txt"))
        .unwrap()
        .meta
        .hash
        .as_ref()
        .unwrap()
        .clone();
    assert_eq!(hash_a, hash_b);

    // Only one blob should exist in the store.
    let objects_dir = repo.objects_dir();
    let mut blob_count = 0;
    for entry in fs::read_dir(&objects_dir).unwrap() {
        let entry = entry.unwrap();
        if entry.path().is_dir() {
            for shard_entry in fs::read_dir(entry.path()).unwrap() {
                shard_entry.unwrap();
                blob_count += 1;
            }
        }
    }
    assert_eq!(blob_count, 1, "identical content should be stored once");
}

#[test]
fn list_returns_all_checkpoints_sorted_by_time() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"data");

    // Create two checkpoints with different timestamps.
    let scanner = Scanner::new(&repo.root);
    let scan_result = scanner.scan().unwrap();

    let meta1 = CheckpointMeta {
        id: varn::core::CheckpointId("p".to_string()),
        description: "first".to_string(),
        created_at: 1000,
        root: repo.root.clone(),
    };
    let snap1 = SnapshotData::new(meta1, scan_result.entries.clone());
    snap1.save(&repo.snapshots_dir()).unwrap();

    let meta2 = CheckpointMeta {
        id: varn::core::CheckpointId("p".to_string()),
        description: "second".to_string(),
        created_at: 2000,
        root: repo.root.clone(),
    };
    let snap2 = SnapshotData::new(meta2, scan_result.entries.clone());
    snap2.save(&repo.snapshots_dir()).unwrap();

    let list = SnapshotData::list_all(&repo.snapshots_dir()).unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].meta.description, "first");
    assert_eq!(list[1].meta.description, "second");
}

#[test]
fn diff_detects_added_file() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"original");

    let snapshot = create_checkpoint(&repo, "before change");

    // Add a new file.
    write_file(tmp.path(), "b.txt", b"new file");

    // Scan current state.
    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();

    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, varn::diff::ChangeKind::Added);
    assert_eq!(changes[0].path, std::path::Path::new("b.txt"));
}

#[test]
fn diff_detects_modified_file() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"original content");

    let snapshot = create_checkpoint(&repo, "before change");

    // Modify the file.
    write_file(tmp.path(), "a.txt", b"modified content");

    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();

    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, varn::diff::ChangeKind::Modified);
    assert_eq!(changes[0].path, std::path::Path::new("a.txt"));
}

#[test]
fn diff_detects_deleted_file() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"keep");
    write_file(tmp.path(), "b.txt", b"delete me");

    let snapshot = create_checkpoint(&repo, "before change");

    // Delete a file.
    fs::remove_file(tmp.path().join("b.txt")).unwrap();

    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();

    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, varn::diff::ChangeKind::Removed);
    assert_eq!(changes[0].path, std::path::Path::new("b.txt"));
}

#[test]
fn diff_detects_combined_changes() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "keep.txt", b"unchanged");
    write_file(tmp.path(), "modify.txt", b"original");
    write_file(tmp.path(), "delete.txt", b"to be deleted");

    let snapshot = create_checkpoint(&repo, "before changes");

    // Make changes.
    write_file(tmp.path(), "modify.txt", b"changed");
    fs::remove_file(tmp.path().join("delete.txt")).unwrap();
    write_file(tmp.path(), "add.txt", b"new file");

    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();

    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert_eq!(changes.len(), 3);

    for change in &changes {
        match change.path.to_string_lossy().as_ref() {
            "modify.txt" => assert_eq!(change.kind, varn::diff::ChangeKind::Modified),
            "delete.txt" => assert_eq!(change.kind, varn::diff::ChangeKind::Removed),
            "add.txt" => assert_eq!(change.kind, varn::diff::ChangeKind::Added),
            other => panic!("unexpected path: {other}"),
        }
    }
}

#[test]
fn diff_no_changes_when_identical() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"stable");
    write_file(tmp.path(), "b.txt", b"also stable");

    let snapshot = create_checkpoint(&repo, "baseline");

    // No changes.
    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();

    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(changes.is_empty());
}

#[test]
fn checkpoint_round_trip_preserves_entries() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"content a");
    write_file(tmp.path(), "sub/b.txt", b"content b");

    let snapshot = create_checkpoint(&repo, "round trip test");
    let id = snapshot.meta.id.0.clone();

    // Load it back.
    let loaded = SnapshotData::load_by_id(&repo.snapshots_dir(), &id).unwrap();
    assert_eq!(snapshot.entries, loaded.entries);
    assert_eq!(snapshot.meta.description, loaded.meta.description);
}

#[test]
fn checkpoint_id_is_deterministic_for_same_content() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"deterministic");

    let scanner = Scanner::new(&repo.root);
    let scan_result = scanner.scan().unwrap();

    let meta1 = CheckpointMeta {
        id: varn::core::CheckpointId("p".to_string()),
        description: "test".to_string(),
        created_at: 5000,
        root: repo.root.clone(),
    };
    let meta2 = CheckpointMeta {
        id: varn::core::CheckpointId("p".to_string()),
        description: "test".to_string(),
        created_at: 5000,
        root: repo.root.clone(),
    };

    let snap1 = SnapshotData::new(meta1, scan_result.entries.clone());
    let snap2 = SnapshotData::new(meta2, scan_result.entries.clone());
    assert_eq!(snap1.meta.id.0, snap2.meta.id.0);
}

#[test]
fn checkpoint_id_differs_for_different_description() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"content");

    let scanner = Scanner::new(&repo.root);
    let scan_result = scanner.scan().unwrap();

    let meta1 = CheckpointMeta {
        id: varn::core::CheckpointId("p".to_string()),
        description: "first".to_string(),
        created_at: 5000,
        root: repo.root.clone(),
    };
    let meta2 = CheckpointMeta {
        id: varn::core::CheckpointId("p".to_string()),
        description: "second".to_string(),
        created_at: 5000,
        root: repo.root.clone(),
    };

    let snap1 = SnapshotData::new(meta1, scan_result.entries.clone());
    let snap2 = SnapshotData::new(meta2, scan_result.entries.clone());
    assert_ne!(snap1.meta.id.0, snap2.meta.id.0);
}

#[test]
fn checkpoint_excludes_varn_directory() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "real.txt", b"data");

    let snapshot = create_checkpoint(&repo, "test");

    // No entry should start with .varn.
    for entry in &snapshot.entries {
        let path_str = entry.path.to_string_lossy();
        assert!(
            !path_str.starts_with(".varn"),
            ".varn should be excluded from snapshots"
        );
    }
}

#[test]
fn checkpoint_stores_nested_files() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a/b/c/deep.txt", b"deep content");

    let snapshot = create_checkpoint(&repo, "nested test");

    let deep_entry = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("a/b/c/deep.txt"))
        .expect("nested file should be in snapshot");
    assert_eq!(deep_entry.meta.kind, EntryKind::File);
    assert!(deep_entry.meta.hash.is_some());

    // Content should be stored.
    let hash = deep_entry.meta.hash.as_ref().unwrap();
    assert!(repo.object_store().exists(hash));
    assert_eq!(
        repo.object_store().read_content(hash).unwrap(),
        b"deep content"
    );
}

#[test]
fn multiple_checkpoints_share_objects() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"shared content");

    // Create two checkpoints of the same state.
    let snap1 = create_checkpoint(&repo, "first");
    let _snap2 = create_checkpoint(&repo, "second");

    // Both should reference the same object.
    let hash = snap1
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("a.txt"))
        .unwrap()
        .meta
        .hash
        .as_ref()
        .unwrap()
        .clone();

    // Object should exist exactly once.
    assert!(repo.object_store().exists(&hash));
    let content = repo.object_store().read_content(&hash).unwrap();
    assert_eq!(content, b"shared content");

    // Verify the hash matches what we expect.
    assert_eq!(hash, hash_bytes(b"shared content"));
}

#[test]
fn snapshot_data_is_serializable() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"serialize me");

    let snapshot = create_checkpoint(&repo, "serialization test");

    // Serialize and deserialize.
    let json = serde_json::to_string(&snapshot).unwrap();
    let back: SnapshotData = serde_json::from_str(&json).unwrap();
    assert_eq!(snapshot, back);
}

#[test]
fn diff_with_nested_directories() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    fs::create_dir_all(tmp.path().join("src/utils")).unwrap();
    write_file(tmp.path(), "src/main.rs", b"fn main() {}");
    write_file(tmp.path(), "src/utils/helper.rs", b"pub fn help() {}");

    let snapshot = create_checkpoint(&repo, "before");

    // Add a new file in a nested directory.
    write_file(tmp.path(), "src/utils/new.rs", b"pub fn new() {}");

    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();

    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, varn::diff::ChangeKind::Added);
    assert_eq!(changes[0].path, std::path::Path::new("src/utils/new.rs"));
}

#[test]
fn checkpoint_with_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    // No files at all, just the .varn directory.

    let snapshot = create_checkpoint(&repo, "empty workspace");

    // Should have zero entries (the .varn directory is excluded).
    assert!(snapshot.entries.is_empty());
}

#[test]
fn checkpoint_with_symlink() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "target.txt", b"target");
    #[cfg(unix)]
    std::os::unix::fs::symlink(tmp.path().join("target.txt"), tmp.path().join("link.txt")).unwrap();
    #[cfg(not(unix))]
    {
        return;
    }

    let snapshot = create_checkpoint(&repo, "symlink test");

    let link = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("link.txt"))
        .expect("symlink should be in snapshot");
    assert_eq!(link.meta.kind, EntryKind::Symlink);
    assert!(link.meta.hash.is_none());
}
