//! Incremental scanning: cache file metadata to skip unchanged files.
//!
//! On each scan, the scanner records each file's path, size, mtime, and
//! content hash in a cache. On the next scan, if a file's size and mtime
//! haven't changed, the cached hash is reused instead of re-reading and
//! re-hashing the file content.
//!
//! The cache is persisted as JSON in `.varn/index/scan_cache.json`.
//!
//! This is safe because:
//! - If a file's content changes, its mtime almost always changes too.
//! - If only the mtime changes (e.g. `touch`), the hash is recomputed.
//! - If the cache is missing or corrupt, a full scan is performed.
//! - The cache is advisory: correctness never depends on it.

use crate::error::Result;
use crate::filesystem::TreeEntry;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Cached metadata for a single file entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CachedEntry {
    /// File size in bytes.
    pub size: u64,
    /// Modification time (unix seconds), if available.
    pub mtime: Option<i64>,
    /// Content hash (SHA-256 hex), if available.
    pub hash: Option<String>,
}

/// A cache of file metadata from a previous scan.
///
/// Maps relative paths (as strings with forward slashes) to cached entry
/// metadata. Used to skip re-hashing unchanged files during incremental
/// scanning.
///
/// # Trust model
///
/// The cache is **advisory only**: it affects performance (whether to re-hash
/// a file), never correctness. Content stored via `store_content_streaming`
/// is independently hash-verified. A poisoned or corrupt cache can cause a
/// stale hash to be reported for a modified file, but this is detected by
/// the hash verification during storage and by diff/restore comparison.
///
/// The cache carries a `version` field so future format changes can
/// invalidate the entire cache by bumping `CACHE_VERSION`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ScanCache {
    /// Cache format version. If this doesn't match `CACHE_VERSION`, the
    /// cache is treated as empty.
    #[serde(default)]
    version: u32,
    /// Map of relative path → cached entry.
    entries: BTreeMap<String, CachedEntry>,
}

/// The current cache format version. Bump to invalidate all existing caches.
const CACHE_VERSION: u32 = 1;

impl ScanCache {
    /// Create an empty cache.
    pub fn new() -> Self {
        Self::default()
    }

    /// Load a cache from a JSON file. Returns an empty cache if the file
    /// does not exist, is corrupt, or has an incompatible version.
    pub fn load(path: &Path) -> Self {
        match fs::read_to_string(path) {
            Ok(data) => {
                let cache: Self = serde_json::from_str(&data).unwrap_or_default();
                // Invalidate caches with an incompatible version.
                if cache.version != CACHE_VERSION {
                    Self::new()
                } else {
                    cache
                }
            }
            Err(_) => Self::new(),
        }
    }

    /// Save the cache to a JSON file.
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)?;
        // Atomic write: temp file then rename.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    /// Look up a cached entry by path.
    pub fn get(&self, path: &str) -> Option<&CachedEntry> {
        self.entries.get(path)
    }

    /// Insert or update a cached entry.
    pub fn insert(&mut self, path: &str, entry: CachedEntry) {
        self.entries.insert(path.to_string(), entry);
    }

    /// Remove a cached entry (e.g. if the file no longer exists).
    pub fn remove(&mut self, path: &str) {
        self.entries.remove(path);
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Rebuild the cache from a list of tree entries.
    pub fn from_entries(entries: &[TreeEntry]) -> Self {
        let mut cache = Self::new();
        cache.version = CACHE_VERSION;
        for entry in entries {
            let path_str = entry.path.to_string_lossy().replace('\\', "/");
            cache.insert(
                &path_str,
                CachedEntry {
                    size: entry.meta.size,
                    mtime: entry.meta.mtime,
                    hash: entry.meta.hash.clone(),
                },
            );
        }
        cache
    }

    /// Check whether a file's cached metadata is still valid (size and
    /// mtime match the current values).
    pub fn is_valid(&self, path: &str, size: u64, mtime: Option<i64>) -> bool {
        match self.get(path) {
            Some(cached) => cached.size == size && cached.mtime == mtime,
            None => false,
        }
    }

    /// Get the cached hash for a file if the cache is still valid.
    pub fn cached_hash(&self, path: &str, size: u64, mtime: Option<i64>) -> Option<&str> {
        if self.is_valid(path, size, mtime) {
            self.get(path).and_then(|e| e.hash.as_deref())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{EntryKind, EntryMeta, TreeEntry};
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn file_entry(path: &str, size: u64, mtime: Option<i64>, hash: Option<&str>) -> TreeEntry {
        TreeEntry {
            path: PathBuf::from(path),
            meta: EntryMeta {
                kind: EntryKind::File,
                size,
                readonly: false,
                mtime,
                hash: hash.map(String::from),
                target: None,
                nlink: 1,
                hardlink_to: None,
                uid: None,
                gid: None,
            },
        }
    }

    #[test]
    fn scan_cache_empty() {
        let cache = ScanCache::new();
        assert!(cache.is_empty());
        assert_eq!(cache.len(), 0);
    }

    #[test]
    fn scan_cache_insert_and_get() {
        let mut cache = ScanCache::new();
        cache.insert(
            "src/main.rs",
            CachedEntry {
                size: 100,
                mtime: Some(1000),
                hash: Some("abc123".to_string()),
            },
        );
        assert_eq!(cache.len(), 1);
        let entry = cache.get("src/main.rs").unwrap();
        assert_eq!(entry.size, 100);
        assert_eq!(entry.mtime, Some(1000));
        assert_eq!(entry.hash.as_deref(), Some("abc123"));
    }

    #[test]
    fn scan_cache_is_valid() {
        let mut cache = ScanCache::new();
        cache.insert(
            "a.txt",
            CachedEntry {
                size: 50,
                mtime: Some(1000),
                hash: Some("hash".to_string()),
            },
        );
        // Valid: same size and mtime.
        assert!(cache.is_valid("a.txt", 50, Some(1000)));
        // Invalid: different size.
        assert!(!cache.is_valid("a.txt", 51, Some(1000)));
        // Invalid: different mtime.
        assert!(!cache.is_valid("a.txt", 50, Some(2000)));
        // Invalid: not in cache.
        assert!(!cache.is_valid("b.txt", 50, Some(1000)));
    }

    #[test]
    fn scan_cache_cached_hash() {
        let mut cache = ScanCache::new();
        cache.insert(
            "a.txt",
            CachedEntry {
                size: 50,
                mtime: Some(1000),
                hash: Some("hash123".to_string()),
            },
        );
        // Valid cache: returns hash.
        assert_eq!(cache.cached_hash("a.txt", 50, Some(1000)), Some("hash123"));
        // Invalid cache: returns None.
        assert_eq!(cache.cached_hash("a.txt", 51, Some(1000)), None);
    }

    #[test]
    fn scan_cache_from_entries() {
        let entries = vec![
            file_entry("a.txt", 10, Some(1000), Some("hash_a")),
            file_entry("b.txt", 20, Some(2000), Some("hash_b")),
        ];
        let cache = ScanCache::from_entries(&entries);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.cached_hash("a.txt", 10, Some(1000)), Some("hash_a"));
        assert_eq!(cache.cached_hash("b.txt", 20, Some(2000)), Some("hash_b"));
    }

    #[test]
    fn scan_cache_save_and_load() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cache.json");
        let mut cache = ScanCache::new();
        cache.version = CACHE_VERSION;
        cache.insert(
            "a.txt",
            CachedEntry {
                size: 10,
                mtime: Some(1000),
                hash: Some("hash".to_string()),
            },
        );
        cache.save(&path).unwrap();
        let loaded = ScanCache::load(&path);
        assert_eq!(loaded, cache);
    }

    #[test]
    fn scan_cache_load_missing_file_returns_empty() {
        let cache = ScanCache::load(std::path::Path::new("/nonexistent/cache.json"));
        assert!(cache.is_empty());
    }

    #[test]
    fn scan_cache_load_corrupt_file_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cache.json");
        std::fs::write(&path, "not valid json {{{").unwrap();
        let cache = ScanCache::load(&path);
        assert!(cache.is_empty());
    }

    #[test]
    fn scan_cache_remove() {
        let mut cache = ScanCache::new();
        cache.insert(
            "a.txt",
            CachedEntry {
                size: 10,
                mtime: Some(1000),
                hash: Some("hash".to_string()),
            },
        );
        assert_eq!(cache.len(), 1);
        cache.remove("a.txt");
        assert_eq!(cache.len(), 0);
        assert!(cache.get("a.txt").is_none());
    }
}
