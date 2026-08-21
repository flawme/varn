//! Restore verification: confirming the filesystem matches a snapshot.
//!
//! After a restore, the filesystem is re-scanned and compared against the
//! snapshot entries. This catches any discrepancy between the intended
//! state and the actual state on disk.

use crate::filesystem::{EntryKind, TreeEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Verify that the current filesystem state matches a snapshot.
///
/// Re-scans the filesystem and compares entries. Returns `true` if the
/// state matches, `false` otherwise.
pub fn verify_restore(root: &Path, snapshot: &[TreeEntry]) -> bool {
    let scanner = crate::filesystem::Scanner::new(root);
    let scan_result = match scanner.scan() {
        Ok(r) => r,
        Err(_) => return false,
    };

    // Compare entry counts.
    if scan_result.entries.len() != snapshot.len() {
        return false;
    }

    // Build a map of snapshot entries for lookup.
    let snap_map: BTreeMap<&PathBuf, &TreeEntry> = snapshot.iter().map(|e| (&e.path, e)).collect();

    // Compare each current entry with the snapshot.
    for entry in &scan_result.entries {
        match snap_map.get(&entry.path) {
            None => return false,
            Some(snap_entry) => {
                // Compare kind.
                if entry.meta.kind != snap_entry.meta.kind {
                    return false;
                }
                // For files, compare hash.
                if entry.meta.kind == EntryKind::File && entry.meta.hash != snap_entry.meta.hash {
                    return false;
                }
                // For symlinks, compare target.
                if entry.meta.kind == EntryKind::Symlink
                    && entry.meta.target != snap_entry.meta.target
                {
                    return false;
                }
                // Compare metadata (readonly, mtime) for all entry types.
                // This catches a restore that failed to set permissions or
                // timestamps correctly.
                if entry.meta.readonly != snap_entry.meta.readonly {
                    return false;
                }
                if entry.meta.mtime != snap_entry.meta.mtime {
                    return false;
                }
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{EntryKind, EntryMeta};
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn file_entry(path: &str, hash: Option<&str>) -> TreeEntry {
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
    fn verify_restore_matching_state() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        fs::create_dir_all(tmp.path().join("sub")).unwrap();

        let scanner = crate::filesystem::Scanner::new(tmp.path());
        let scan = scanner.scan().unwrap();

        assert!(verify_restore(tmp.path(), &scan.entries));
    }

    #[test]
    fn verify_restore_mismatched_state() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();

        // Snapshot says "a.txt" has hash X, but the file has different content.
        let snapshot = vec![file_entry("a.txt", Some("wrong_hash"))];

        assert!(!verify_restore(tmp.path(), &snapshot));
    }

    #[test]
    fn verify_restore_extra_file() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.txt"), b"hello").unwrap();
        fs::write(tmp.path().join("b.txt"), b"extra").unwrap();

        // Snapshot only has "a.txt".
        let snapshot = vec![file_entry("a.txt", Some("any"))];

        assert!(!verify_restore(tmp.path(), &snapshot));
    }

    #[test]
    fn verify_restore_missing_file() {
        let tmp = TempDir::new().unwrap();
        // No files on disk.
        let snapshot = vec![file_entry("a.txt", Some("any"))];

        assert!(!verify_restore(tmp.path(), &snapshot));
    }
}
