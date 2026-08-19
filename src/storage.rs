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
        let obj_path = self.object_path(hash);
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
        self.object_path(hash).exists()
    }

    /// Read an object's content.
    pub fn read_content(&self, hash: &str) -> Result<Vec<u8>> {
        let path = self.object_path(hash);
        if !path.exists() {
            return Err(VarnError::Other(format!("object not found: {hash}")));
        }
        Ok(fs::read(&path)?)
    }

    /// Compute the on-disk path for a given hash.
    ///
    /// Uses a 2-character shard: `ab/cdef1234...`
    fn object_path(&self, hash: &str) -> PathBuf {
        let (shard, rest) = if hash.len() >= 2 {
            hash.split_at(2)
        } else {
            (hash, "")
        };
        self.dir.join(shard).join(rest)
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
        let err = store.read_content("nonexistent").unwrap_err();
        assert!(matches!(err, VarnError::Other(_)));
    }

    #[test]
    fn repo_provides_object_store() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();
        store.store_content("abcdef", b"data").unwrap();
        assert!(store.exists("abcdef"));
    }
}
