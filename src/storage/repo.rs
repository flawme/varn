//! Repository configuration, initialization, and discovery.

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
    pub fn object_store(&self) -> crate::storage::ObjectStore {
        crate::storage::ObjectStore::new(&self.objects_dir())
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
}
