//! Diff engine: comparing two filesystem states.
//!
//! This module is a placeholder for the full diff engine.

use crate::filesystem::{EntryKind, TreeEntry};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The kind of change detected for a single path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeKind {
    /// The path exists in the new state but not the old.
    Added,
    /// The path exists in both but its metadata differs.
    Modified,
    /// The path exists in the old state but not the new.
    Removed,
}

/// A single change between two states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// What kind of change this is.
    pub kind: ChangeKind,
    /// The affected path.
    pub path: PathBuf,
}

/// Compute the set of changes between an old state and a new state.
///
/// Entries are matched by path. This is a pure function over pre-collected
/// entry lists and does not touch the filesystem.
///
/// Directory modification times are intentionally excluded from the comparison:
/// a directory's mtime changes whenever a child is added or removed, so it is
/// a side-effect of other detected changes rather than an independent content
/// change. Permission changes on directories are still detected.
pub fn diff_states(old: &[TreeEntry], new: &[TreeEntry]) -> Vec<Change> {
    let old_map: BTreeMap<&PathBuf, &TreeEntry> = old.iter().map(|e| (&e.path, e)).collect();
    let new_map: BTreeMap<&PathBuf, &TreeEntry> = new.iter().map(|e| (&e.path, e)).collect();

    let mut changes = Vec::new();

    // Added and modified.
    for (path, new_entry) in &new_map {
        match old_map.get(path) {
            None => changes.push(Change {
                kind: ChangeKind::Added,
                path: (*path).clone(),
            }),
            Some(old_entry) => {
                if !entries_equal(old_entry, new_entry) {
                    changes.push(Change {
                        kind: ChangeKind::Modified,
                        path: (*path).clone(),
                    })
                }
            }
        }
    }

    // Removed.
    for path in old_map.keys() {
        if !new_map.contains_key(path) {
            changes.push(Change {
                kind: ChangeKind::Removed,
                path: (*path).clone(),
            })
        }
    }

    changes
}

/// Compare two entries for diff purposes.
///
/// This is like `PartialEq` but excludes the modification time and size of
/// directories. A directory's mtime is updated by the OS whenever a child entry
/// is added or removed, so it reflects other changes already captured by
/// separate entries rather than an independent modification of the directory
/// itself. The same holds for a directory's reported size: on some platforms
/// (notably macOS) it grows with the number of child entries, while on others
/// (Linux) it is a fixed block-aligned constant. Either way it is a side-effect
/// of child add/remove, not an independent content change. Ignoring both
/// prevents spurious "Modified" reports on parent directories when a file is
/// added or deleted inside them.
fn entries_equal(old: &TreeEntry, new: &TreeEntry) -> bool {
    if old.meta.kind != new.meta.kind {
        return false;
    }
    // For directories, skip mtime and size comparison.
    if old.meta.kind == EntryKind::Directory {
        old.meta.readonly == new.meta.readonly
            && old.meta.hash == new.meta.hash
            && old.meta.target == new.meta.target
    } else {
        old.meta == new.meta
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{EntryKind, EntryMeta};
    use std::path::PathBuf;

    fn entry(path: &str, size: u64) -> TreeEntry {
        TreeEntry {
            path: PathBuf::from(path),
            meta: EntryMeta {
                kind: EntryKind::File,
                size,
                readonly: false,
                mtime: None,
                hash: None,
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
    fn diff_empty_states() {
        let changes = diff_states(&[], &[]);
        assert!(changes.is_empty());
    }

    #[test]
    fn diff_detects_added() {
        let old = vec![entry("a", 1)];
        let new = vec![entry("a", 1), entry("b", 2)];
        let changes = diff_states(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Added);
        assert_eq!(changes[0].path, PathBuf::from("b"));
    }

    #[test]
    fn diff_detects_modified() {
        let old = vec![entry("a", 1)];
        let new = vec![entry("a", 2)];
        let changes = diff_states(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
        assert_eq!(changes[0].path, PathBuf::from("a"));
    }

    #[test]
    fn diff_detects_removed() {
        let old = vec![entry("a", 1), entry("b", 2)];
        let new = vec![entry("a", 1)];
        let changes = diff_states(&old, &new);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Removed);
        assert_eq!(changes[0].path, PathBuf::from("b"));
    }

    #[test]
    fn diff_no_changes_when_identical() {
        let old = vec![entry("a", 1), entry("b", 2)];
        let new = vec![entry("a", 1), entry("b", 2)];
        let changes = diff_states(&old, &new);
        assert!(changes.is_empty());
    }

    #[test]
    fn diff_combined_changes() {
        let old = vec![entry("a", 1), entry("b", 2), entry("c", 3)];
        let new = vec![entry("a", 1), entry("b", 9), entry("d", 4)];
        let changes = diff_states(&old, &new);
        // b modified, c removed, d added.
        assert_eq!(changes.len(), 3);
        let mut kinds: Vec<_> = changes
            .iter()
            .map(|c| (c.path.to_string_lossy().to_string(), c.kind.clone()))
            .collect();
        kinds.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(kinds[0], ("b".to_string(), ChangeKind::Modified));
        assert_eq!(kinds[1], ("c".to_string(), ChangeKind::Removed));
        assert_eq!(kinds[2], ("d".to_string(), ChangeKind::Added));
    }

    #[test]
    fn diff_ignores_directory_mtime_change() {
        // A directory's mtime changes when a child is added/removed, but that
        // is a side-effect of other entries — it must not produce a spurious
        // "Modified" on the directory itself.
        let dir_old = TreeEntry {
            path: PathBuf::from("src"),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 0,
                readonly: false,
                mtime: Some(1000),
                hash: None,
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
        };
        let dir_new = TreeEntry {
            path: PathBuf::from("src"),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 0,
                readonly: false,
                mtime: Some(2000), // mtime changed
                hash: None,
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
        };
        let file_old = entry("src/main.rs", 1);
        let file_new = entry("src/main.rs", 1);

        let changes = diff_states(&[dir_old, file_old], &[dir_new, file_new]);
        assert!(
            changes.is_empty(),
            "directory mtime change alone should not be a modification"
        );
    }

    #[test]
    fn diff_detects_directory_permission_change() {
        // Permission changes on a directory ARE meaningful and must be detected.
        let dir_old = TreeEntry {
            path: PathBuf::from("src"),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 0,
                readonly: false,
                mtime: Some(1000),
                hash: None,
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
        };
        let dir_new = TreeEntry {
            path: PathBuf::from("src"),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 0,
                readonly: true, // permission changed
                mtime: Some(1000),
                hash: None,
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
        };
        let changes = diff_states(&[dir_old], &[dir_new]);
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].kind, ChangeKind::Modified);
    }

    #[test]
    fn diff_ignores_directory_size_change() {
        // On some platforms (notably macOS) a directory's reported size grows
        // as child entries are added. Like mtime, this is a side-effect of
        // child add/remove and must not produce a spurious "Modified" on the
        // directory itself.
        let dir_old = TreeEntry {
            path: PathBuf::from("src/utils"),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 64,
                readonly: false,
                mtime: Some(1000),
                hash: None,
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
        };
        let dir_new = TreeEntry {
            path: PathBuf::from("src/utils"),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 96, // size grew (child added)
                readonly: false,
                mtime: Some(2000),
                hash: None,
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
        };
        let file_old = entry("src/utils/helper.rs", 1);
        let file_new = entry("src/utils/helper.rs", 1);

        let changes = diff_states(&[dir_old, file_old], &[dir_new, file_new]);
        assert!(
            changes.is_empty(),
            "directory size change alone should not be a modification"
        );
    }
}
