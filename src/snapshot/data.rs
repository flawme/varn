//! Snapshot data model and persistence.
//!
//! A snapshot is the persisted representation of a filesystem state at a
//! point in time. It consists of:
//!
//! - [`CheckpointMeta`] — identity, description, timestamp, root path
//! - A list of [`TreeEntry`] records — the captured filesystem state
//!
//! Snapshots are persisted as JSON files in `.varn/snapshots/<id>.json`.
//! File contents are stored separately in the content-addressed object
//! store (see [`crate::storage::ObjectStore`]).

use crate::core::{CheckpointId, CheckpointMeta};
use crate::error::{Result, VarnError};
use crate::filesystem::TreeEntry;
use crate::snapshot::id::{generate_checkpoint_id, is_valid_id};
use crate::storage::ObjectStore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// A complete snapshot: checkpoint metadata plus the captured filesystem
/// state.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Metadata describing the checkpoint.
    pub meta: CheckpointMeta,
    /// The entries captured in this snapshot, in canonical order.
    pub entries: Vec<TreeEntry>,
}

/// The serializable representation of a snapshot, persisted to disk as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotData {
    /// Metadata describing the checkpoint.
    pub meta: CheckpointMeta,
    /// The entries captured in this snapshot, sorted by path.
    pub entries: Vec<TreeEntry>,
}

impl SnapshotData {
    /// The file extension for snapshot files.
    pub const EXTENSION: &'static str = "json";

    /// Create a new `SnapshotData` from metadata and entries.
    ///
    /// Entries are sorted by path to ensure deterministic serialization.
    pub fn new(mut meta: CheckpointMeta, mut entries: Vec<TreeEntry>) -> Self {
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        // Generate the checkpoint ID from the content hash of the entries.
        let id = generate_checkpoint_id(&meta, &entries);
        meta.id = CheckpointId(id);
        Self { meta, entries }
    }

    /// Check whether a snapshot with the given ID already exists on disk.
    pub fn exists(snapshots_dir: &Path, id: &str) -> bool {
        if !is_valid_id(id) {
            return false;
        }
        snapshots_dir
            .join(format!("{id}.{}", Self::EXTENSION))
            .exists()
    }

    /// Serialize and write the snapshot to `<snapshots_dir>/<id>.json`.
    ///
    /// Uses an atomic temp-file-then-rename strategy.
    ///
    /// If a snapshot with the same ID already exists, it is **not**
    /// overwritten — the existing file is kept and `false` is returned.
    /// This makes checkpointing idempotent: the same state checkpointed
    /// twice produces the same ID and does not duplicate or overwrite.
    pub fn save(&self, snapshots_dir: &Path) -> Result<bool> {
        if !is_valid_id(&self.meta.id.0) {
            return Err(VarnError::InvalidPath(format!(
                "invalid checkpoint id: {}",
                self.meta.id.0
            )));
        }
        fs::create_dir_all(snapshots_dir)?;
        let filename = format!("{}.{}", self.meta.id.0, Self::EXTENSION);
        let path = snapshots_dir.join(filename);
        if path.exists() {
            return Ok(false);
        }
        let json = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        Ok(true)
    }

    /// Load a snapshot from a JSON file.
    pub fn load(path: &Path) -> Result<Self> {
        let data = fs::read_to_string(path)?;
        let snapshot: Self = serde_json::from_str(&data)?;
        Ok(snapshot)
    }

    /// Load a snapshot by its checkpoint ID from a snapshots directory.
    pub fn load_by_id(snapshots_dir: &Path, id: &str) -> Result<Self> {
        if !is_valid_id(id) {
            return Err(VarnError::InvalidPath(format!(
                "invalid checkpoint id: {id}"
            )));
        }
        let filename = format!("{id}.{}", Self::EXTENSION);
        let path = snapshots_dir.join(filename);
        if !path.exists() {
            return Err(VarnError::Other(format!("checkpoint not found: {id}")));
        }
        Self::load(&path)
    }

    /// List all snapshot files in a snapshots directory, returning their
    /// parsed data sorted by creation time (oldest first).
    pub fn list_all(snapshots_dir: &Path) -> Result<Vec<Self>> {
        if !snapshots_dir.exists() {
            return Ok(Vec::new());
        }
        let mut snapshots = Vec::new();
        for entry in fs::read_dir(snapshots_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some(Self::EXTENSION) {
                match Self::load(&path) {
                    Ok(s) => snapshots.push(s),
                    Err(e) => {
                        // Skip corrupt snapshots but don't abort the list.
                        eprintln!("warning: could not read snapshot {}: {e}", path.display());
                    }
                }
            }
        }
        snapshots.sort_by_key(|s| s.meta.created_at);
        Ok(snapshots)
    }

    /// Collect the set of object hashes referenced by this snapshot's file
    /// entries.
    ///
    /// Only regular file entries with a content hash are included. Directories,
    /// symlinks, and entries without a hash are skipped.
    pub fn referenced_hashes(&self) -> std::collections::HashSet<&str> {
        self.entries
            .iter()
            .filter_map(|e| e.meta.hash.as_deref())
            .collect()
    }

    /// Store file content blobs into the object store.
    ///
    /// For each file entry with a hash, the content is read from the source
    /// root and stored in the object store. This enables deduplication and
    /// restoration.
    ///
    /// If the file's content has changed since the scan (its hash no longer
    /// matches), a warning is emitted and the entry's hash is cleared to
    /// prevent storing inconsistent data.
    pub fn store_content_blobs(&self, source_root: &Path, store: &ObjectStore) -> Result<()> {
        for entry in &self.entries {
            if let Some(ref hash) = entry.meta.hash {
                if store.exists(hash) {
                    continue;
                }
                // Validate the entry path is safe (no traversal outside root).
                if entry.path.is_absolute()
                    || entry.path.components().any(|c| {
                        matches!(
                            c,
                            std::path::Component::ParentDir
                                | std::path::Component::RootDir
                                | std::path::Component::Prefix(_)
                        )
                    })
                {
                    return Err(VarnError::InvalidPath(format!(
                        "unsafe entry path in snapshot: {}",
                        entry.path.display()
                    )));
                }
                let full_path = source_root.join(&entry.path);
                let content = fs::read(&full_path).map_err(|e| {
                    VarnError::Other(format!(
                        "cannot read file for storage: {}: {e}",
                        full_path.display()
                    ))
                })?;
                // Verify the content matches the hash from the scan.
                // If it doesn't, the file was modified between scan and store.
                let actual_hash = crate::filesystem::hash_bytes(&content);
                if actual_hash != *hash {
                    return Err(VarnError::Other(format!(
                        "file changed during checkpoint: {} (expected hash {}, got {})",
                        entry.path.display(),
                        hash,
                        actual_hash
                    )));
                }
                store.store_content(hash, &content)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{CheckpointId, CheckpointMeta};
    use crate::filesystem::{EntryKind, EntryMeta};
    use crate::storage::Repo;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_meta() -> CheckpointMeta {
        CheckpointMeta {
            id: CheckpointId("placeholder".to_string()),
            description: "test checkpoint".to_string(),
            created_at: 1_700_000_000,
            root: PathBuf::from("/tmp/project"),
        }
    }

    fn make_entry(path: &str, hash: Option<&str>) -> TreeEntry {
        TreeEntry {
            path: PathBuf::from(path),
            meta: EntryMeta {
                kind: EntryKind::File,
                size: 10,
                readonly: false,
                mtime: None,
                hash: hash.map(String::from),
                target: None,
            },
        }
    }

    #[test]
    fn snapshot_holds_meta_and_entries() {
        let snap = Snapshot {
            meta: CheckpointMeta {
                id: CheckpointId("a91f".to_string()),
                description: "test".to_string(),
                created_at: 1,
                root: PathBuf::from("/tmp"),
            },
            entries: vec![],
        };
        assert_eq!(snap.meta.id.0, "a91f");
        assert!(snap.entries.is_empty());
    }

    #[test]
    fn snapshot_data_generates_id_from_content() {
        let meta = make_meta();
        let entries = vec![make_entry("a.txt", Some("abc123"))];
        let data = SnapshotData::new(meta, entries);
        assert_eq!(data.meta.id.0.len(), 12);
        assert!(data.meta.id.0.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn snapshot_data_id_is_deterministic() {
        let meta = make_meta();
        let entries = vec![
            make_entry("a.txt", Some("abc123")),
            make_entry("b.txt", Some("def456")),
        ];
        let data1 = SnapshotData::new(meta.clone(), entries.clone());
        let data2 = SnapshotData::new(meta, entries);
        assert_eq!(data1.meta.id.0, data2.meta.id.0);
    }

    #[test]
    fn snapshot_data_id_differs_for_different_content() {
        let meta = make_meta();
        let entries1 = vec![make_entry("a.txt", Some("abc123"))];
        let entries2 = vec![make_entry("a.txt", Some("xyz789"))];
        let data1 = SnapshotData::new(meta.clone(), entries1);
        let data2 = SnapshotData::new(meta, entries2);
        assert_ne!(data1.meta.id.0, data2.meta.id.0);
    }

    #[test]
    fn snapshot_data_sorts_entries() {
        let meta = make_meta();
        let entries = vec![
            make_entry("z.txt", None),
            make_entry("a.txt", None),
            make_entry("m.txt", None),
        ];
        let data = SnapshotData::new(meta, entries);
        assert_eq!(data.entries[0].path, PathBuf::from("a.txt"));
        assert_eq!(data.entries[1].path, PathBuf::from("m.txt"));
        assert_eq!(data.entries[2].path, PathBuf::from("z.txt"));
    }

    #[test]
    fn snapshot_data_serialization_round_trip() {
        let meta = make_meta();
        let entries = vec![
            make_entry("a.txt", Some("abc123")),
            make_entry("b.txt", None),
        ];
        let data = SnapshotData::new(meta, entries);
        let json = serde_json::to_string(&data).unwrap();
        let back: SnapshotData = serde_json::from_str(&json).unwrap();
        assert_eq!(data, back);
    }

    #[test]
    fn snapshot_save_and_load_round_trip() {
        let tmp = TempDir::new().unwrap();
        let snapshots_dir = tmp.path().join("snapshots");

        let meta = make_meta();
        let entries = vec![make_entry("a.txt", Some("abc123"))];
        let data = SnapshotData::new(meta, entries);
        let id = data.meta.id.0.clone();

        let saved = data.save(&snapshots_dir).unwrap();
        assert!(saved, "first save should report true");
        let loaded = SnapshotData::load_by_id(&snapshots_dir, &id).unwrap();
        assert_eq!(data, loaded);
    }

    #[test]
    fn snapshot_save_is_idempotent_for_duplicate() {
        let tmp = TempDir::new().unwrap();
        let snapshots_dir = tmp.path().join("snapshots");

        let meta = make_meta();
        let entries = vec![make_entry("a.txt", Some("abc123"))];
        let data = SnapshotData::new(meta, entries);

        let saved1 = data.save(&snapshots_dir).unwrap();
        assert!(saved1);

        let saved2 = data.save(&snapshots_dir).unwrap();
        assert!(!saved2, "duplicate save should report false");

        let list = SnapshotData::list_all(&snapshots_dir).unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn snapshot_exists_checks_disk() {
        let tmp = TempDir::new().unwrap();
        let snapshots_dir = tmp.path().join("snapshots");

        let meta = make_meta();
        let entries = vec![make_entry("a.txt", Some("abc123"))];
        let data = SnapshotData::new(meta, entries);
        let id = data.meta.id.0.clone();

        assert!(!SnapshotData::exists(&snapshots_dir, &id));
        data.save(&snapshots_dir).unwrap();
        assert!(SnapshotData::exists(&snapshots_dir, &id));
    }

    #[test]
    fn snapshot_load_by_id_missing_fails() {
        let tmp = TempDir::new().unwrap();
        let err = SnapshotData::load_by_id(tmp.path(), "abcdef123456").unwrap_err();
        assert!(matches!(err, VarnError::Other(_)));
    }

    #[test]
    fn snapshot_load_by_id_rejects_invalid_id() {
        let tmp = TempDir::new().unwrap();
        let err = SnapshotData::load_by_id(tmp.path(), "nonexistent").unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));
    }

    #[test]
    fn snapshot_list_all_returns_sorted_by_time() {
        let tmp = TempDir::new().unwrap();
        let snapshots_dir = tmp.path().join("snapshots");

        let meta1 = CheckpointMeta {
            id: CheckpointId("p".to_string()),
            description: "first".to_string(),
            created_at: 1000,
            root: PathBuf::from("/tmp"),
        };
        let meta2 = CheckpointMeta {
            id: CheckpointId("p".to_string()),
            description: "second".to_string(),
            created_at: 2000,
            root: PathBuf::from("/tmp"),
        };

        let data1 = SnapshotData::new(meta1, vec![]);
        let data2 = SnapshotData::new(meta2, vec![]);
        data1.save(&snapshots_dir).unwrap();
        data2.save(&snapshots_dir).unwrap();

        let list = SnapshotData::list_all(&snapshots_dir).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].meta.description, "first");
        assert_eq!(list[1].meta.description, "second");
    }

    #[test]
    fn snapshot_list_all_empty_dir() {
        let tmp = TempDir::new().unwrap();
        let list = SnapshotData::list_all(tmp.path()).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn snapshot_list_all_nonexistent_dir() {
        let list = SnapshotData::list_all(Path::new("/nonexistent/path")).unwrap();
        assert!(list.is_empty());
    }

    #[test]
    fn store_content_blobs_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let repo = Repo::init(root, "linux").unwrap();
        let store = repo.object_store();

        std::fs::write(root.join("a.txt"), b"same").unwrap();
        std::fs::write(root.join("b.txt"), b"same").unwrap();

        let hash = crate::filesystem::hash_bytes(b"same");
        let entries = vec![
            TreeEntry {
                path: PathBuf::from("a.txt"),
                meta: EntryMeta {
                    kind: EntryKind::File,
                    size: 4,
                    readonly: false,
                    mtime: None,
                    hash: Some(hash.clone()),
                    target: None,
                },
            },
            TreeEntry {
                path: PathBuf::from("b.txt"),
                meta: EntryMeta {
                    kind: EntryKind::File,
                    size: 4,
                    readonly: false,
                    mtime: None,
                    hash: Some(hash.clone()),
                    target: None,
                },
            },
        ];

        let meta = CheckpointMeta {
            id: CheckpointId("p".to_string()),
            description: "test".to_string(),
            created_at: 1,
            root: root.to_path_buf(),
        };
        let data = SnapshotData::new(meta, entries);
        data.store_content_blobs(root, &store).unwrap();

        assert!(store.exists(&hash));
        assert_eq!(store.read_content(&hash).unwrap(), b"same");
    }

    #[test]
    fn store_content_blobs_skips_entries_without_hash() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let repo = Repo::init(root, "linux").unwrap();
        let store = repo.object_store();

        std::fs::create_dir_all(root.join("subdir")).unwrap();

        let entries = vec![TreeEntry {
            path: PathBuf::from("subdir"),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 0,
                readonly: false,
                mtime: None,
                hash: None,
                target: None,
            },
        }];

        let meta = CheckpointMeta {
            id: CheckpointId("p".to_string()),
            description: "test".to_string(),
            created_at: 1,
            root: root.to_path_buf(),
        };
        let data = SnapshotData::new(meta, entries);
        data.store_content_blobs(root, &store).unwrap();
    }

    #[test]
    fn referenced_hashes_collects_file_hashes() {
        let meta = make_meta();
        let entries = vec![
            make_entry("a.txt", Some("hash_a")),
            make_entry("b.txt", Some("hash_b")),
            make_entry("c.txt", None),
        ];
        let data = SnapshotData::new(meta, entries);
        let hashes = data.referenced_hashes();
        assert_eq!(hashes.len(), 2);
        assert!(hashes.contains("hash_a"));
        assert!(hashes.contains("hash_b"));
    }

    #[test]
    fn referenced_hashes_deduplicates_identical() {
        let meta = make_meta();
        let entries = vec![
            make_entry("a.txt", Some("same_hash")),
            make_entry("b.txt", Some("same_hash")),
        ];
        let data = SnapshotData::new(meta, entries);
        let hashes = data.referenced_hashes();
        assert_eq!(hashes.len(), 1);
        assert!(hashes.contains("same_hash"));
    }

    #[test]
    fn load_by_id_rejects_traversal() {
        let tmp = TempDir::new().unwrap();
        let err = SnapshotData::load_by_id(tmp.path(), "../../../etc/passwd").unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));
    }
}
