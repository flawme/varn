//! Garbage collection: removing unreferenced objects from the store.

use crate::error::Result;
use crate::storage::repo::Repo;
use std::collections::HashSet;

/// The result of a garbage collection operation.
#[derive(Debug, Clone)]
pub struct GcResult {
    /// Total number of objects in the store before GC.
    pub total_objects: usize,
    /// Number of objects referenced by at least one snapshot.
    pub referenced_objects: usize,
    /// Number of unreferenced objects that were deleted.
    pub deleted: usize,
    /// Hashes of the deleted objects.
    pub deleted_hashes: Vec<String>,
}

/// Run garbage collection on a repository's object store.
///
/// Deletes objects that are not referenced by any snapshot. This is safe to
/// run at any time — objects referenced by any existing snapshot are kept.
///
/// The `dry_run` flag controls whether objects are actually deleted. When
/// `true`, the result reports what *would* be deleted without deleting.
pub fn garbage_collect(repo: &Repo, dry_run: bool) -> Result<GcResult> {
    // Collect all hashes referenced by all snapshots.
    let snapshots = crate::snapshot::SnapshotData::list_all(&repo.snapshots_dir())?;
    let mut referenced: HashSet<String> = HashSet::new();
    for snap in &snapshots {
        for hash in snap.referenced_hashes() {
            referenced.insert(hash.to_string());
        }
    }

    // List all objects in the store.
    let store = repo.object_store();
    let all_objects = store.list_objects()?;

    // Find unreferenced objects.
    let mut deleted_hashes = Vec::new();
    for hash in &all_objects {
        if !referenced.contains(hash) {
            if !dry_run {
                store.delete_object(hash)?;
            }
            deleted_hashes.push(hash.clone());
        }
    }

    Ok(GcResult {
        total_objects: all_objects.len(),
        referenced_objects: referenced.len(),
        deleted: deleted_hashes.len(),
        deleted_hashes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CheckpointId, CheckpointMeta};
    use crate::filesystem::{EntryKind, EntryMeta, TreeEntry};
    use crate::snapshot::SnapshotData;
    use crate::storage::Repo;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn gc_deletes_unreferenced_objects() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Store an object that will be referenced.
        store.store_content("aaaa1111", b"referenced").unwrap();
        // Store an object that no snapshot references.
        store.store_content("bbbb2222", b"unreferenced").unwrap();

        // Create a snapshot that references "aaaa1111".
        let meta = CheckpointMeta {
            id: CheckpointId("p".to_string()),
            description: "test".to_string(),
            created_at: 1000,
            root: repo.root.clone(),
        };
        let entries = vec![TreeEntry {
            path: PathBuf::from("a.txt"),
            meta: EntryMeta {
                kind: EntryKind::File,
                size: 10,
                readonly: false,
                mtime: None,
                hash: Some("aaaa1111".to_string()),
                target: None,
                nlink: 1,
                hardlink_to: None,
                uid: None,
                gid: None,
            },
        }];
        let snap = SnapshotData::new(meta, entries);
        snap.save(&repo.snapshots_dir()).unwrap();

        // Run GC.
        let result = garbage_collect(&repo, false).unwrap();
        assert_eq!(result.total_objects, 2);
        assert_eq!(result.referenced_objects, 1);
        assert_eq!(result.deleted, 1);
        assert!(result.deleted_hashes.contains(&"bbbb2222".to_string()));

        // The referenced object should still exist.
        assert!(store.exists("aaaa1111"));
        // The unreferenced object should be gone.
        assert!(!store.exists("bbbb2222"));
    }

    #[test]
    fn garbage_collect_dry_run_does_not_delete() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        store.store_content("aaaa1111", b"referenced").unwrap();
        store.store_content("bbbb2222", b"unreferenced").unwrap();

        // No snapshots at all — both objects are unreferenced.
        let result = garbage_collect(&repo, true).unwrap();
        assert_eq!(result.deleted, 2);
        // Dry run: nothing should be deleted.
        assert!(store.exists("aaaa1111"));
        assert!(store.exists("bbbb2222"));
    }

    #[test]
    fn garbage_collect_with_no_snapshots_deletes_all() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        store.store_content("aaaa1111", b"a").unwrap();
        store.store_content("bbbb2222", b"b").unwrap();

        let result = garbage_collect(&repo, false).unwrap();
        assert_eq!(result.deleted, 2);
        assert!(!store.exists("aaaa1111"));
        assert!(!store.exists("bbbb2222"));
    }

    #[test]
    fn garbage_collect_keeps_objects_referenced_by_multiple_snapshots() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        store.store_content("5abee1112222", b"shared").unwrap();

        // Two snapshots both reference the same object.
        let make_snap = |desc: &str, created_at: i64| {
            let meta = CheckpointMeta {
                id: CheckpointId("p".to_string()),
                description: desc.to_string(),
                created_at,
                root: repo.root.clone(),
            };
            let entries = vec![TreeEntry {
                path: PathBuf::from("a.txt"),
                meta: EntryMeta {
                    kind: EntryKind::File,
                    size: 6,
                    readonly: false,
                    mtime: None,
                    hash: Some("5abee1112222".to_string()),
                    target: None,
                    nlink: 1,
                    hardlink_to: None,
                    uid: None,
                    gid: None,
                },
            }];
            let snap = SnapshotData::new(meta, entries);
            snap.save(&repo.snapshots_dir()).unwrap();
        };

        make_snap("first", 1000);
        make_snap("second", 2000);

        let result = garbage_collect(&repo, false).unwrap();
        assert_eq!(result.deleted, 0);
        assert!(store.exists("5abee1112222"));
    }
}
