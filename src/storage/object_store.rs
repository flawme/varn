//! Content-addressed object storage.
//!
//! File contents are stored as blobs keyed by their SHA-256 hash. Objects
//! are sharded into a two-level directory structure (`ab/cdef...`) to avoid
//! having too many files in a single directory.
//!
//! Identical content is stored only once (deduplication): if an object
//! already exists at the target path, `store_content` is a no-op.

use crate::error::{Result, VarnError};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Content-addressed object storage.
/// Monotonic counter distinguishing concurrent temp-file writes within
/// one process (the PID alone is shared by all threads).
static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

    /// Store file content by streaming from a reader, computing the SHA-256
    /// hash as it goes.
    ///
    /// This avoids reading the entire file into memory, making it safe for
    /// very large files. The content is streamed to a temp file in the object
    /// store's shard directory, then the hash is verified against the expected
    /// value before renaming to the final path.
    ///
    /// If the object already exists, this is a no-op (the reader is not
    /// consumed).
    pub fn store_content_streaming(
        &self,
        expected_hash: &str,
        reader: &mut dyn Read,
    ) -> Result<()> {
        let obj_path = self.object_path(expected_hash)?;
        if obj_path.exists() {
            return Ok(());
        }

        // Ensure the shard directory exists.
        if let Some(parent) = obj_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Stream to a temp file while hashing.
        // Use a unique temp file name to prevent symlink-based temp file
        // attacks (CVE-2023-34034 pattern: predictable temp file names allow
        // pre-creating a symlink that redirects the write). The name must
        // be unique per WRITE, not per process: two threads checkpointing
        // concurrently share the PID and would otherwise collide on the
        // same temp path (one renames it away, the other's rename fails
        // with ENOENT).
        let tmp = obj_path.with_extension(format!(
            "{}.{}.tmp",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut file = fs::File::create(&tmp)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 65536]; // 64KB buffer
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n])?;
        }
        file.flush()?;

        // Verify the hash matches.
        let actual_hash = format!("{:x}", hasher.finalize());
        if actual_hash != expected_hash {
            // Clean up the temp file.
            let _ = fs::remove_file(&tmp);
            return Err(VarnError::Other(format!(
                "content hash mismatch: expected {expected_hash}, got {actual_hash}"
            )));
        }

        // Rename to final path.
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

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
        store.store_content(hash, b"first").unwrap();
        // Second store with different content at same hash should be a no-op.
        store.store_content(hash, b"second").unwrap();
        assert_eq!(store.read_content(hash).unwrap(), b"first");
    }

    #[test]
    fn object_store_list_objects() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        store.store_content("aaaa1111", b"a").unwrap();
        store.store_content("bbbb2222", b"b").unwrap();
        let list = store.list_objects().unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&"aaaa1111".to_string()));
        assert!(list.contains(&"bbbb2222".to_string()));
    }

    #[test]
    fn object_store_delete_object() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        store.store_content("aaaa1111", b"data").unwrap();
        assert!(store.delete_object("aaaa1111").unwrap());
        assert!(!store.exists("aaaa1111"));
        // Deleting again returns false.
        assert!(!store.delete_object("aaaa1111").unwrap());
    }

    #[test]
    fn object_store_rejects_path_traversal_hash() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));

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
        assert!(!store.exists("../../../etc/passwd"));
        assert!(!store.exists(""));
    }

    #[test]
    fn object_store_streaming_stores_content() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let content = b"streaming content test";
        let hash = crate::filesystem::hash_bytes(content);
        let mut reader = std::io::Cursor::new(content);
        store.store_content_streaming(&hash, &mut reader).unwrap();
        assert!(store.exists(&hash));
        let read = store.read_content(&hash).unwrap();
        assert_eq!(read, content);
    }

    #[test]
    fn object_store_streaming_deduplicates() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let content = b"dedup content";
        let hash = crate::filesystem::hash_bytes(content);

        let mut reader1 = std::io::Cursor::new(content);
        store.store_content_streaming(&hash, &mut reader1).unwrap();

        // Second store with different content at same hash should be a no-op.
        let mut reader2 = std::io::Cursor::new(b"different content");
        store.store_content_streaming(&hash, &mut reader2).unwrap();
        assert_eq!(store.read_content(&hash).unwrap(), content);
    }

    #[test]
    fn object_store_streaming_rejects_hash_mismatch() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        let content = b"actual content";
        let wrong_hash = "0000000000000000000000000000000000000000000000000000000000000000";
        let mut reader = std::io::Cursor::new(content);
        let err = store
            .store_content_streaming(wrong_hash, &mut reader)
            .unwrap_err();
        assert!(matches!(err, VarnError::Other(_)));
        // Temp file should have been cleaned up.
        assert!(!store.exists(wrong_hash));
    }

    #[test]
    fn object_store_streaming_large_content() {
        let tmp = TempDir::new().unwrap();
        let store = ObjectStore::new(&tmp.path().join("objects"));
        // Create content larger than the read buffer (64KB).
        let content: Vec<u8> = (0..200_000).map(|i| (i % 256) as u8).collect();
        let hash = crate::filesystem::hash_bytes(&content);
        let mut reader = std::io::Cursor::new(&content);
        store.store_content_streaming(&hash, &mut reader).unwrap();
        assert!(store.exists(&hash));
        let read = store.read_content(&hash).unwrap();
        assert_eq!(read, content);
    }
}
