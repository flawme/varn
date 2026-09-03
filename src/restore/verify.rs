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
    // `hash: null` is an old malformed snapshot representation for an
    // unreadable regular file. It does not describe restorable content, so
    // it can never be a verified restore even if a same-named file happens
    // to exist on disk.
    if snapshot.iter().any(|entry| {
        entry.meta.kind == EntryKind::File && entry.meta.hash.as_deref().is_none_or(str::is_empty)
    }) {
        return false;
    }

    let scanner = crate::filesystem::Scanner::new(root);
    let scan_result = match scanner.scan() {
        Ok(r) => r,
        Err(_) => return false,
    };

    // Every snapshot entry must have a matching entry on disk. New
    // checkpoints omit unreadable files rather than persisting `hash: null`;
    // this strict count also makes an older, hashless snapshot fail honestly
    // instead of reporting a false successful restore.
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
                // For files, compare hash. Hashless file entries were
                // rejected above because they cannot be restored or
                // verified.
                if entry.meta.kind == EntryKind::File && entry.meta.hash != snap_entry.meta.hash {
                    return false;
                }
                // For symlinks and junctions, compare target.
                if matches!(entry.meta.kind, EntryKind::Symlink | EntryKind::Junction)
                    && entry.meta.target != snap_entry.meta.target
                {
                    return false;
                }
                // Compare metadata (readonly, mtime, mode, platform metadata)
                // for all entry types. This catches a restore that failed to
                // set permissions, timestamps, or platform metadata correctly.
                if entry.meta.readonly != snap_entry.meta.readonly {
                    return false;
                }
                if entry.meta.mtime != snap_entry.meta.mtime {
                    return false;
                }
                // Full mode (Unix): only enforced when the snapshot captured
                // one (older snapshots fall back to the readonly check).
                if snap_entry.meta.mode.is_some() && entry.meta.mode != snap_entry.meta.mode {
                    return false;
                }
                // Platform-specific metadata, same rule.
                if snap_entry.meta.flags.is_some() && entry.meta.flags != snap_entry.meta.flags {
                    return false;
                }
                if snap_entry.meta.attributes.is_some()
                    && entry.meta.attributes != snap_entry.meta.attributes
                {
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
                nlink: 1,
                hardlink_to: None,
                uid: None,
                gid: None,
                mode: None,
                flags: None,
                attributes: None,
                acl: None,
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
