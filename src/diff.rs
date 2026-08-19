//! Diff engine: comparing two filesystem states.
//!
//! This module is a placeholder for the full diff engine.

use crate::filesystem::TreeEntry;
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
                if old_entry != new_entry {
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
}
