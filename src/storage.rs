//! Storage layer: on-disk layout and persistence for Varn.
//!
//! The repository layout is:
//!
//! ```text
//! .varn/
//! ├── config.toml       (future)
//! ├── config.json       (current config)
//! ├── objects/          (content-addressed blobs, future)
//! ├── snapshots/        (snapshot metadata, future)
//! └── index/            (fast lookups, future)
//! ```
//!
//! Only `config.json` is written by the initial foundation. The other
//! directories are created empty so the layout is established and stable.

use crate::error::{Result, VarnError};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

/// The directory name Varn uses to store its metadata.
pub const VARN_DIR: &str = ".varn";

/// The current storage format version. Increment when the on-disk format
/// changes in a way that requires migration.
pub const STORAGE_VERSION: u32 = 1;

/// Subdirectories created inside `.varn/`.
const SUBDIRS: &[&str] = &["objects", "snapshots", "index"];

/// Repository configuration, persisted as JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoConfig {
    /// Storage format version.
    pub version: u32,
    /// Absolute path of the root directory Varn manages.
    pub root: PathBuf,
    /// Creation timestamp (seconds since UNIX epoch).
    pub created_at: i64,
    /// Platform the repository was created on.
    pub platform: String,
}

impl RepoConfig {
    /// The filename used for the config file inside `.varn/`.
    pub const FILENAME: &'static str = "config.json";

    /// Serialize and write the config to `<varn_dir>/config.json`.
    pub fn write(&self, varn_dir: &Path) -> Result<()> {
        let path = varn_dir.join(Self::FILENAME);
        let json = serde_json::to_string_pretty(self)?;
        // Write atomically: write to a temp file then rename.
        let tmp = varn_dir.join(format!("{}.tmp", Self::FILENAME));
        fs::write(&tmp, json)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Read and deserialize the config from `<varn_dir>/config.json`.
    pub fn read(varn_dir: &Path) -> Result<Self> {
        let path = varn_dir.join(Self::FILENAME);
        let data = fs::read_to_string(&path)?;
        let config: Self = serde_json::from_str(&data)?;
        Ok(config)
    }
}

/// A handle to an initialized Varn repository.
#[derive(Debug, Clone)]
pub struct Repo {
    /// The managed root directory (the parent of `.varn/`).
    pub root: PathBuf,
    /// The path to `.varn/`.
    pub varn_dir: PathBuf,
    /// The loaded configuration.
    pub config: RepoConfig,
}

impl Repo {
    /// Create a new repository at `root`. Fails if already initialized.
    ///
    /// This creates the `.varn/` directory, subdirectories, and writes the
    /// config file. It does not modify anything outside `.varn/`.
    pub fn init(root: &Path, platform: &str) -> Result<Self> {
        let varn_dir = root.join(VARN_DIR);
        if varn_dir.exists() {
            return Err(VarnError::AlreadyInitialized { path: varn_dir });
        }

        fs::create_dir(&varn_dir)?;
        for subdir in SUBDIRS {
            fs::create_dir_all(varn_dir.join(subdir))?;
        }

        let config = RepoConfig {
            version: STORAGE_VERSION,
            root: root.to_path_buf(),
            created_at: now_unix(),
            platform: platform.to_string(),
        };
        config.write(&varn_dir)?;

        Ok(Self {
            root: root.to_path_buf(),
            varn_dir,
            config,
        })
    }

    /// Open an existing repository by searching upward from `start` for a
    /// `.varn/` directory.
    pub fn open(start: &Path) -> Result<Self> {
        let varn_dir = find_varn_dir(start)?;
        let config = RepoConfig::read(&varn_dir)?;
        Ok(Self {
            root: config.root.clone(),
            varn_dir,
            config,
        })
    }

    /// Check whether a `.varn/` directory exists at `root`.
    pub fn exists_at(root: &Path) -> bool {
        root.join(VARN_DIR).exists()
    }

    /// Path to the `objects/` directory inside `.varn/`.
    pub fn objects_dir(&self) -> PathBuf {
        self.varn_dir.join("objects")
    }

    /// Path to the `snapshots/` directory inside `.varn/`.
    pub fn snapshots_dir(&self) -> PathBuf {
        self.varn_dir.join("snapshots")
    }

    /// Get an [`ObjectStore`] backed by this repository's `objects/` directory.
    pub fn object_store(&self) -> ObjectStore {
        ObjectStore::new(&self.objects_dir())
    }
}

/// Content-addressed object storage.
///
/// File contents are stored as blobs keyed by their SHA-256 hash. Objects
/// are sharded into a two-level directory structure (`ab/cdef...`) to avoid
/// having too many files in a single directory.
///
/// Identical content is stored only once (deduplication): if an object
/// already exists at the target path, `store_content` is a no-op.
pub struct ObjectStore {
    /// The root directory for object storage (`.varn/objects/`).
    dir: PathBuf,
}

impl ObjectStore {
    /// Create a new object store rooted at `dir`.
    pub fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_path_buf(),
        }
    }

    /// Store a content blob if it does not already exist.
    ///
    /// The blob is written to `<dir>/<first 2 hex chars>/<remaining hex>`
    /// using an atomic temp-file-then-rename strategy. If the object already
    /// exists, this is a no-op.
    pub fn store_content(&self, hash: &str, content: &[u8]) -> Result<()> {
        let obj_path = self.object_path(hash)?;
        if obj_path.exists() {
            return Ok(());
        }

        // Ensure the shard directory exists.
        if let Some(parent) = obj_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Write to a temp file, then rename for atomicity.
        let tmp = obj_path.with_extension("tmp");
        fs::write(&tmp, content)?;
        fs::rename(&tmp, &obj_path)?;
        Ok(())
    }

    /// Check whether an object exists.
    pub fn exists(&self, hash: &str) -> bool {
        // Silently return false for invalid hashes rather than propagating
        // an error — this is a query, not an operation.
        match self.object_path(hash) {
            Ok(path) => path.exists(),
            Err(_) => false,
        }
    }

    /// Read an object's content.
    pub fn read_content(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.object_path(hash)?;
        if !path.exists() {
            return Err(VarnError::Other(format!("object not found: {hash}")));
        }
        Ok(fs::read(&path)?)
    }

    /// List all object hashes currently stored in the object store.
    ///
    /// Walks the shard directories and reconstructs each object's hash from
    /// its path (`<shard>/<rest>` → `<shard><rest>`). Returns hashes sorted
    /// for deterministic output.
    pub fn list_objects(&self) -> Result<Vec<String>> {
        let mut hashes = Vec::new();
        if !self.dir.exists() {
            return Ok(hashes);
        }
        for shard_entry in fs::read_dir(&self.dir)? {
            let shard_entry = shard_entry?;
            if !shard_entry.file_type()?.is_dir() {
                continue;
            }
            let shard_name = shard_entry.file_name();
            let shard_str = match shard_name.to_str() {
                Some(s) => s,
                None => continue,
            };
            for obj_entry in fs::read_dir(shard_entry.path())? {
                let obj_entry = obj_entry?;
                if !obj_entry.file_type()?.is_file() {
                    continue;
                }
                // Skip temp files.
                let obj_name = obj_entry.file_name();
                let obj_str = match obj_name.to_str() {
                    Some(s) => s,
                    None => continue,
                };
                if obj_str.ends_with(".tmp") {
                    continue;
                }
                // Reconstruct the full hash: shard + rest.
                let hash = format!("{shard_str}{obj_str}");
                hashes.push(hash);
            }
        }
        hashes.sort();
        Ok(hashes)
    }

    /// Delete an object by its hash. Returns `true` if an object was deleted,
    /// `false` if it did not exist.
    pub fn delete_object(&self, hash: &str) -> Result<bool> {
        let path = self.object_path(hash)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(VarnError::Io(e)),
        }
    }

    /// Compute the on-disk path for a given hash.
    ///
    /// Uses a 2-character shard: `ab/cdef1234...`
    ///
    /// Returns an error if the hash contains characters that could escape
    /// the objects directory (path traversal). Only lowercase hexadecimal
    /// characters are accepted.
    fn object_path(&self, hash: &str) -> Result<PathBuf> {
        if hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(VarnError::InvalidPath(format!(
                "invalid object hash (must be hex): {hash}"
            )));
        }
        let (shard, rest) = if hash.len() >= 2 {
            hash.split_at(2)
        } else {
            (hash, "")
        };
        Ok(self.dir.join(shard).join(rest))
    }
}

/// Search upward from `start` for the first ancestor containing `.varn/`.
/// Returns the path to the `.varn/` directory, or an error naming the last
/// searched path if none is found.
pub fn find_varn_dir(start: &Path) -> Result<PathBuf> {
    let start = if start.is_absolute() {
        start.to_path_buf()
    } else {
        fs::canonicalize(start).unwrap_or_else(|_| start.to_path_buf())
    };

    let mut current: Option<&Path> = Some(&start);
    while let Some(dir) = current {
        let candidate = dir.join(VARN_DIR);
        if candidate.is_dir() {
            return Ok(candidate);
        }
        current = dir.parent();
    }

    Err(VarnError::NotInitialized { searched: start })
}

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
    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
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

/// Current time as seconds since the UNIX epoch.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn init_creates_layout() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let repo = Repo::init(root, "linux").unwrap();
        assert!(repo.varn_dir.is_dir());
        assert!(repo.varn_dir.join("objects").is_dir());
        assert!(repo.varn_dir.join("snapshots").is_dir());
        assert!(repo.varn_dir.join("index").is_dir());
        assert!(repo.varn_dir.join(RepoConfig::FILENAME).is_file());
    }

    #[test]
    fn init_fails_if_already_initialized() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        Repo::init(root, "linux").unwrap();
        let err = Repo::init(root, "linux").unwrap_err();
        assert!(matches!(err, VarnError::AlreadyInitialized { .. }));
    }

    #[test]
    fn config_round_trip() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let repo = Repo::init(root, "linux").unwrap();
        let loaded = RepoConfig::read(&repo.varn_dir).unwrap();
        assert_eq!(loaded, repo.config);
        assert_eq!(loaded.version, STORAGE_VERSION);
        assert_eq!(loaded.platform, "linux");
    }

    #[test]
    fn open_finds_repo_from_subdirectory() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let repo = Repo::init(root, "linux").unwrap();
        let sub = root.join("a/b/c");
        fs::create_dir_all(&sub).unwrap();
        let opened = Repo::open(&sub).unwrap();
        assert_eq!(opened.varn_dir, repo.varn_dir);
        assert_eq!(opened.config, repo.config);
    }

    #[test]
    fn open_fails_when_not_initialized() {
        let tmp = TempDir::new().unwrap();
        let err = Repo::open(tmp.path()).unwrap_err();
        assert!(matches!(err, VarnError::NotInitialized { .. }));
    }

    #[test]
    fn exists_at_detects_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        assert!(!Repo::exists_at(root));
        Repo::init(root, "linux").unwrap();
        assert!(Repo::exists_at(root));
    }

    #[test]
    fn find_varn_dir_searches_upward() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        Repo::init(root, "linux").unwrap();
        let deep = root.join("x/y/z");
        fs::create_dir_all(&deep).unwrap();
        let found = find_varn_dir(&deep).unwrap();
        assert_eq!(found, root.join(VARN_DIR));
    }

    #[test]
    fn object_store_stores_and_reads_content() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let hash = "abcdef1234567890";
        let content = b"hello world";
        store.store_content(hash, content).unwrap();
        assert!(store.exists(hash));
        let read = store.read_content(hash).unwrap();
        assert_eq!(read, content);
    }

    #[test]
    fn object_store_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let hash = "abcdef1234567890";
        store.store_content(hash, b"data").unwrap();
        // Storing again must not error and must not duplicate.
        store.store_content(hash, b"data").unwrap();
        assert!(store.exists(hash));
        assert_eq!(store.read_content(hash).unwrap(), b"data");
    }

    #[test]
    fn object_store_shards_by_first_two_chars() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let hash = "abcdef1234567890";
        store.store_content(hash, b"x").unwrap();
        // Should be at objects/ab/cdef1234567890
        let expected = tmp.path().join("objects/ab/cdef1234567890");
        assert!(expected.exists());
    }

    #[test]
    fn object_store_read_missing_fails() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let err = store.read_content("abcdef123456").unwrap_err();
        assert!(matches!(err, VarnError::Other(_)));
    }

    #[test]
    fn object_store_read_invalid_hash_rejected() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let err = store.read_content("nonexistent").unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));
    }

    #[test]
    fn repo_provides_object_store() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();
        store.store_content("abcdef", b"data").unwrap();
        assert!(store.exists("abcdef"));
    }

    #[test]
    fn list_objects_empty_store() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let objects = store.list_objects().unwrap();
        assert!(objects.is_empty());
    }

    #[test]
    fn list_objects_returns_all_hashes() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        store.store_content("abcdef1234", b"a").unwrap();
        store.store_content("bbceef5678", b"b").unwrap();
        store.store_content("ab12345678", b"c").unwrap();

        let objects = store.list_objects().unwrap();
        assert_eq!(objects.len(), 3);
        assert!(objects.contains(&"abcdef1234".to_string()));
        assert!(objects.contains(&"bbceef5678".to_string()));
        assert!(objects.contains(&"ab12345678".to_string()));
    }

    #[test]
    fn list_objects_is_sorted() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        store.store_content("ff1234", b"a").unwrap();
        store.store_content("aa1234", b"b").unwrap();
        store.store_content("cc1234", b"c").unwrap();

        let objects = store.list_objects().unwrap();
        assert_eq!(objects, vec!["aa1234", "cc1234", "ff1234"]);
    }

    #[test]
    fn delete_object_removes_existing() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        store.store_content("abcdef1234", b"data").unwrap();
        assert!(store.exists("abcdef1234"));

        let deleted = store.delete_object("abcdef1234").unwrap();
        assert!(deleted);
        assert!(!store.exists("abcdef1234"));
    }

    #[test]
    fn delete_object_missing_returns_false() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let deleted = store.delete_object("abcdef123456").unwrap();
        assert!(!deleted);
    }

    #[test]
    fn garbage_collect_deletes_unreferenced() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Store an object that will be referenced by a snapshot.
        store.store_content("aaaa1111", b"referenced").unwrap();
        // Store an object that no snapshot references.
        store.store_content("bbbb2222", b"unreferenced").unwrap();

        // Create a snapshot that references "aaaa1111".
        let meta = crate::core::CheckpointMeta {
            id: crate::core::CheckpointId("p".to_string()),
            description: "test".to_string(),
            created_at: 1000,
            root: repo.root.clone(),
        };
        let entries = vec![crate::filesystem::TreeEntry {
            path: std::path::PathBuf::from("a.txt"),
            meta: crate::filesystem::EntryMeta {
                kind: crate::filesystem::EntryKind::File,
                size: 10,
                readonly: false,
                mtime: None,
                hash: Some("aaaa1111".to_string()),
                target: None,
            },
        }];
        let snap = crate::snapshot::SnapshotData::new(meta, entries);
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
            let meta = crate::core::CheckpointMeta {
                id: crate::core::CheckpointId("p".to_string()),
                description: desc.to_string(),
                created_at,
                root: repo.root.clone(),
            };
            let entries = vec![crate::filesystem::TreeEntry {
                path: std::path::PathBuf::from("a.txt"),
                meta: crate::filesystem::EntryMeta {
                    kind: crate::filesystem::EntryKind::File,
                    size: 6,
                    readonly: false,
                    mtime: None,
                    hash: Some("5abee1112222".to_string()),
                    target: None,
                },
            }];
            let snap = crate::snapshot::SnapshotData::new(meta, entries);
            snap.save(&repo.snapshots_dir()).unwrap();
        };

        make_snap("first", 1000);
        make_snap("second", 2000);

        let result = garbage_collect(&repo, false).unwrap();
        assert_eq!(result.deleted, 0);
        assert!(store.exists("5abee1112222"));
    }

    #[test]
    fn object_store_rejects_path_traversal_hash() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));

        // Hash with path traversal characters should be rejected.
        let err = store
            .store_content("../../../etc/passwd", b"malicious")
            .unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));

        let err = store.read_content("../../etc/shadow").unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));

        let err = store.delete_object("/etc/passwd").unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));
    }

    #[test]
    fn object_store_exists_returns_false_for_invalid_hash() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        // Should not panic — just return false.
        assert!(!store.exists("../../../etc/passwd"));
        assert!(!store.exists(""));
    }
}
