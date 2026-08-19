//! Filesystem data model.
//!
//! Types describing entries discovered while scanning a directory tree.
//! These are the building blocks for snapshots and diffs.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// The kind of a filesystem entry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Any other entry type (sockets, fifos, block/char devices).
    Other,
}

impl EntryKind {
    /// Map a [`std::fs::FileType`] to an [`EntryKind`].
    pub fn from_file_type(ft: fs::FileType) -> Self {
        if ft.is_file() {
            Self::File
        } else if ft.is_dir() {
            Self::Directory
        } else if ft.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }
}

/// Metadata for a single filesystem entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntryMeta {
    /// Kind of entry (file, directory, symlink, other).
    pub kind: EntryKind,
    /// File size in bytes (0 for directories and symlinks).
    pub size: u64,
    /// Whether the entry is read-only (no write permission for owner).
    pub readonly: bool,
    /// Modification time as seconds since the UNIX epoch, if available.
    pub mtime: Option<i64>,
    /// Content hash (SHA-256) for regular files, or `None` for directories,
    /// symlinks, and other entry types.
    pub hash: Option<String>,
}

/// A single entry in a scanned tree, relative to the scan root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeEntry {
    /// Path relative to the scan root, using forward slashes.
    pub path: PathBuf,
    /// Metadata for the entry.
    pub meta: EntryMeta,
}

// ---------------------------------------------------------------------------
// Scanner
// ---------------------------------------------------------------------------

use crate::error::Result;
use crate::platform;
use crate::storage::VARN_DIR;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;

/// A non-fatal warning encountered during scanning.
///
/// Scanning continues past warnings so that a single inaccessible file does
/// not abort the entire scan. All warnings are collected and returned in the
/// [`ScanResult`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanWarning {
    /// The path that triggered the warning (relative to the scan root).
    pub path: PathBuf,
    /// Human-readable description of the problem.
    pub message: String,
}

/// The outcome of scanning a directory tree.
#[derive(Debug, Clone)]
pub struct ScanResult {
    /// All discovered entries, sorted by path.
    pub entries: Vec<TreeEntry>,
    /// Non-fatal warnings collected during the scan.
    pub warnings: Vec<ScanWarning>,
}

/// A recursive directory scanner that produces [`TreeEntry`] lists with
/// content hashes.
///
/// The scanner:
/// - Uses `symlink_metadata` so symlinks are recorded as symlinks, not
///   followed.
/// - Computes SHA-256 content hashes for regular files.
/// - Skips the `.varn/` directory at the scan root.
/// - Sorts entries by path for deterministic output.
/// - Collects per-entry errors as warnings instead of aborting.
pub struct Scanner {
    /// The root directory to scan.
    root: PathBuf,
}

impl Scanner {
    /// Create a new scanner for the given root directory.
    pub fn new(root: &Path) -> Self {
        Self {
            root: root.to_path_buf(),
        }
    }

    /// Run the scan and return all entries plus any warnings.
    pub fn scan(&self) -> Result<ScanResult> {
        let mut entries = Vec::new();
        let mut warnings = Vec::new();
        self.scan_dir(&self.root.clone(), &mut entries, &mut warnings)?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(ScanResult { entries, warnings })
    }

    /// Recursively scan `dir`, appending entries and warnings.
    fn scan_dir(
        &self,
        dir: &Path,
        entries: &mut Vec<TreeEntry>,
        warnings: &mut Vec<ScanWarning>,
    ) -> Result<()> {
        let read = match fs::read_dir(dir) {
            Ok(r) => r,
            Err(e) => {
                let rel = self.relative_path(dir);
                warnings.push(ScanWarning {
                    path: rel,
                    message: format!("cannot read directory: {e}"),
                });
                return Ok(());
            }
        };

        for entry in read {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    warnings.push(ScanWarning {
                        path: self.relative_path(dir),
                        message: format!("cannot read directory entry: {e}"),
                    });
                    continue;
                }
            };

            let full = entry.path();
            let name = entry.file_name();

            // Skip the .varn directory at the scan root.
            if dir == self.root && name == std::ffi::OsStr::new(VARN_DIR) {
                continue;
            }

            let rel = self.relative_path(&full);

            // Use symlink_metadata so we see the link itself, not its target.
            let meta = match fs::symlink_metadata(&full) {
                Ok(m) => m,
                Err(e) => {
                    // File may have disappeared between readdir and stat.
                    warnings.push(ScanWarning {
                        path: rel,
                        message: format!("cannot read metadata: {e}"),
                    });
                    continue;
                }
            };

            let kind = EntryKind::from_file_type(meta.file_type());

            let hash = if kind == EntryKind::File {
                match self.hash_file(&full) {
                    Ok(h) => Some(h),
                    Err(e) => {
                        warnings.push(ScanWarning {
                            path: rel.clone(),
                            message: format!("cannot hash file: {e}"),
                        });
                        None
                    }
                }
            } else {
                None
            };

            entries.push(TreeEntry {
                path: rel.clone(),
                meta: EntryMeta {
                    kind,
                    size: meta.len(),
                    readonly: platform::is_readonly_meta(&meta),
                    mtime: mtime_to_unix(&meta),
                    hash,
                },
            });

            // Recurse into directories.
            if kind == EntryKind::Directory {
                self.scan_dir(&full, entries, warnings)?;
            }
        }

        Ok(())
    }

    /// Compute the SHA-256 hash of a file's contents as a hex string.
    fn hash_file(&self, path: &Path) -> Result<String> {
        let mut file = fs::File::open(path)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let digest = hasher.finalize();
        Ok(format!("{digest:x}"))
    }

    /// Compute the path of `full` relative to the scan root, using forward
    /// slashes for cross-platform consistency.
    fn relative_path(&self, full: &Path) -> PathBuf {
        match full.strip_prefix(&self.root) {
            Ok(rel) => rel.to_path_buf(),
            Err(_) => full.to_path_buf(),
        }
    }
}

/// Extract the modification time from metadata as seconds since the UNIX
/// epoch, if available.
fn mtime_to_unix(meta: &fs::Metadata) -> Option<i64> {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
}

/// Compute the SHA-256 hash of a byte slice as a hex string.
///
/// This is a public utility for testing and verification.
pub fn hash_bytes(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    let digest = hasher.finalize();
    format!("{digest:x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn entry_kind_from_file_type_file() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("f.txt");
        fs::write(&f, b"hello").unwrap();
        let meta = fs::symlink_metadata(&f).unwrap();
        let kind = EntryKind::from_file_type(meta.file_type());
        assert_eq!(kind, EntryKind::File);
    }

    #[test]
    fn entry_kind_from_file_type_dir() {
        let tmp = TempDir::new().unwrap();
        let meta = fs::symlink_metadata(tmp.path()).unwrap();
        let kind = EntryKind::from_file_type(meta.file_type());
        assert_eq!(kind, EntryKind::Directory);
    }

    #[test]
    fn entry_kind_from_file_type_symlink() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target");
        fs::write(&target, b"x").unwrap();
        let link = tmp.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(not(unix))]
        {
            // Symlinks require privileges on Windows; skip the assertion there.
            return;
        }
        let meta = fs::symlink_metadata(&link).unwrap();
        let kind = EntryKind::from_file_type(meta.file_type());
        assert_eq!(kind, EntryKind::Symlink);
    }

    #[test]
    fn tree_entry_serialization_round_trip() {
        let entry = TreeEntry {
            path: PathBuf::from("src/main.rs"),
            meta: EntryMeta {
                kind: EntryKind::File,
                size: 42,
                readonly: false,
                mtime: Some(1_700_000_000),
                hash: Some(
                    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_string(),
                ),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: TreeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn entry_kind_serializes_lowercase() {
        let json = serde_json::to_string(&EntryKind::Directory).unwrap();
        assert_eq!(json, r#""directory""#);
    }

    #[test]
    fn hash_bytes_empty() {
        assert_eq!(
            hash_bytes(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn hash_bytes_hello() {
        assert_eq!(
            hash_bytes(b"hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn scanner_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let result = Scanner::new(tmp.path()).scan().unwrap();
        assert!(result.entries.is_empty());
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn scanner_finds_single_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].path, PathBuf::from("a.txt"));
        assert_eq!(result.entries[0].meta.kind, EntryKind::File);
        assert_eq!(result.entries[0].meta.size, 5);
        assert_eq!(
            result.entries[0].meta.hash.as_deref(),
            Some("2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824")
        );
    }

    #[test]
    fn scanner_finds_nested_directories() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("a/b/c")).unwrap();
        fs::write(tmp.path().join("a/b/c/deep.txt"), b"deep").unwrap();
        fs::write(tmp.path().join("a/top.txt"), b"top").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        assert_eq!(result.entries.len(), 5);

        let paths: Vec<_> = result
            .entries
            .iter()
            .map(|e| e.path.to_string_lossy().to_string())
            .collect();
        assert!(paths.contains(&"a".to_string()));
        assert!(paths.contains(&"a/b".to_string()));
        assert!(paths.contains(&"a/b/c".to_string()));
        assert!(paths.contains(&"a/b/c/deep.txt".to_string()));
        assert!(paths.contains(&"a/top.txt".to_string()));
    }

    #[test]
    fn scanner_entries_are_sorted() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("zebra.txt"), b"z").unwrap();
        fs::write(tmp.path().join("alpha.txt"), b"a").unwrap();
        fs::write(tmp.path().join("middle.txt"), b"m").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let paths: Vec<_> = result
            .entries
            .iter()
            .map(|e| e.path.to_string_lossy().to_string())
            .collect();
        assert_eq!(paths, vec!["alpha.txt", "middle.txt", "zebra.txt"]);
    }

    #[test]
    fn scanner_skips_varn_directory() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("real.txt"), b"data").unwrap();
        fs::create_dir_all(tmp.path().join(VARN_DIR).join("objects")).unwrap();
        fs::write(tmp.path().join(VARN_DIR).join("config.json"), b"{}").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let paths: Vec<_> = result
            .entries
            .iter()
            .map(|e| e.path.to_string_lossy().to_string())
            .collect();
        assert!(paths.iter().all(|p| !p.starts_with(".varn")));
        assert_eq!(paths, vec!["real.txt"]);
    }

    #[test]
    fn scanner_hashes_match_content() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"identical content").unwrap();
        fs::write(tmp.path().join("b.txt"), b"identical content").unwrap();
        fs::write(tmp.path().join("c.txt"), b"different content").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let hash_a = find_entry(&result, "a.txt").meta.hash.as_deref().unwrap();
        let hash_b = find_entry(&result, "b.txt").meta.hash.as_deref().unwrap();
        let hash_c = find_entry(&result, "c.txt").meta.hash.as_deref().unwrap();

        assert_eq!(hash_a, hash_b, "identical files must have identical hashes");
        assert_ne!(hash_a, hash_c, "different files must have different hashes");
    }

    #[test]
    fn scanner_records_symlink_as_symlink() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("target.txt"), b"target").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(tmp.path().join("target.txt"), tmp.path().join("link.txt"))
            .unwrap();
        #[cfg(not(unix))]
        {
            return;
        }

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let link = find_entry(&result, "link.txt");
        assert_eq!(link.meta.kind, EntryKind::Symlink);
        assert_eq!(link.meta.hash, None, "symlinks should not be hashed");
    }

    #[test]
    fn scanner_handles_empty_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("empty.txt"), b"").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let entry = find_entry(&result, "empty.txt");
        assert_eq!(entry.meta.size, 0);
        assert_eq!(
            entry.meta.hash.as_deref(),
            Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
    }

    #[test]
    fn scanner_handles_unicode_filenames() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("café.txt"), b"coffee").unwrap();
        fs::write(tmp.path().join("日本語.txt"), b"japanese").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        assert!(find_entry(&result, "café.txt").meta.kind == EntryKind::File);
        assert!(find_entry(&result, "日本語.txt").meta.kind == EntryKind::File);
    }

    #[test]
    fn scanner_handles_spaces_in_filenames() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("my file.txt"), b"spaced").unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].path, PathBuf::from("my file.txt"));
    }

    #[test]
    fn scanner_records_directories() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("subdir")).unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let dir = find_entry(&result, "subdir");
        assert_eq!(dir.meta.kind, EntryKind::Directory);
        assert_eq!(dir.meta.hash, None, "directories should not be hashed");
    }

    #[test]
    fn scanner_collects_warning_for_unreadable_file() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("locked.txt");
        fs::write(&f, b"secret").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&f).unwrap().permissions();
            perms.set_mode(0o000);
            fs::set_permissions(&f, perms).unwrap();
        }

        let result = Scanner::new(tmp.path()).scan().unwrap();

        #[cfg(unix)]
        {
            // The file should still appear in entries (metadata was readable)
            // but its hash should be None due to the read error.
            let entry = find_entry(&result, "locked.txt");
            assert_eq!(entry.meta.kind, EntryKind::File);
            assert!(entry.meta.hash.is_none());
            assert!(
                result
                    .warnings
                    .iter()
                    .any(|w| { w.path == Path::new("locked.txt") && w.message.contains("hash") })
            );
        }
        #[cfg(not(unix))]
        {
            let _ = result;
        }
    }

    /// Helper: find an entry by its relative path string.
    fn find_entry<'a>(result: &'a ScanResult, name: &str) -> &'a TreeEntry {
        result
            .entries
            .iter()
            .find(|e| e.path == Path::new(name))
            .unwrap_or_else(|| panic!("entry not found: {name}"))
    }
}
