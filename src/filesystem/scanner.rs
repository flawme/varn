//! Directory scanner: walks a directory tree and produces [`TreeEntry`] lists.

use crate::error::Result;
use crate::filesystem::types::{EntryKind, EntryMeta, TreeEntry};
use crate::platform;
use crate::storage::VARN_DIR;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

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
/// - Skips the `.varn/` directory at any depth.
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

            // Skip the .varn directory at any depth, not just the scan root.
            // This prevents checkpointing nested .varn directories (e.g. from
            // a subdirectory that was separately initialized).
            if name == std::ffi::OsStr::new(VARN_DIR) {
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

            // For symlinks, capture the target path so it can be restored.
            let target = if kind == EntryKind::Symlink {
                match fs::read_link(&full) {
                    Ok(t) => Some(t),
                    Err(e) => {
                        warnings.push(ScanWarning {
                            path: rel.clone(),
                            message: format!("cannot read symlink target: {e}"),
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
                    target,
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
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
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
    }

    #[test]
    fn scanner_finds_nested_directories() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("a/b/c")).unwrap();
        fs::write(tmp.path().join("a/b/c/file.txt"), b"nested").unwrap();
        let result = Scanner::new(tmp.path()).scan().unwrap();
        assert_eq!(result.entries.len(), 4);
    }

    #[test]
    fn scanner_entries_are_sorted_by_path() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("c.txt"), b"").unwrap();
        fs::write(tmp.path().join("a.txt"), b"").unwrap();
        fs::write(tmp.path().join("b.txt"), b"").unwrap();
        let result = Scanner::new(tmp.path()).scan().unwrap();
        let paths: Vec<_> = result.entries.iter().map(|e| &e.path).collect();
        assert_eq!(
            paths,
            vec![
                &PathBuf::from("a.txt"),
                &PathBuf::from("b.txt"),
                &PathBuf::from("c.txt"),
            ]
        );
    }

    #[test]
    fn scanner_skips_varn_directory() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join(".varn")).unwrap();
        fs::write(tmp.path().join(".varn/config.json"), b"{}").unwrap();
        fs::write(tmp.path().join("real.txt"), b"real").unwrap();
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
        assert_eq!(
            link.meta.target.as_deref(),
            Some(tmp.path().join("target.txt").as_path()),
            "symlink target should be captured"
        );
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
