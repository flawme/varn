//! Restore plan types and planning logic.
//!
//! The plan is a pure function over pre-collected entry lists — it does
//! not touch the filesystem. It identifies every action needed to bring
//! the current state back to the snapshot, plus any conflicts that require
//! user confirmation.

use crate::filesystem::{EntryKind, TreeEntry};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Validate that a relative path is safe to join to the restore root.
///
/// Rejects absolute paths and paths containing `..` components that could
/// escape the root directory. This prevents path traversal attacks via
/// malicious snapshot data.
pub fn is_safe_relative_path(path: &Path) -> bool {
    if path.is_absolute() {
        return false;
    }
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

/// A conflict detected during restore planning.
///
/// A conflict means the current filesystem state differs from the snapshot
/// in a way that would cause data loss during restore. The user must
/// confirm before proceeding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// A file was modified since the checkpoint and would be overwritten.
    Modified { path: PathBuf },
    /// A file exists now but not in the checkpoint, and would be deleted.
    Unexpected { path: PathBuf },
}

/// An action that the restore engine will perform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreAction {
    /// Write or overwrite a file with content from the object store.
    WriteFile {
        path: PathBuf,
        hash: String,
        /// Whether the file should be read-only after restore.
        readonly: bool,
        /// Modification time to restore (unix seconds), if available.
        mtime: Option<i64>,
    },
    /// Create a directory.
    CreateDir {
        path: PathBuf,
        /// Whether the directory should be read-only after restore.
        readonly: bool,
        /// Modification time to restore (unix seconds), if available.
        mtime: Option<i64>,
    },
    /// Create a symbolic link pointing to `target`.
    CreateSymlink { path: PathBuf, target: PathBuf },
    /// Delete a file or directory that exists now but not in the checkpoint.
    Delete { path: PathBuf },
}

/// A complete restore plan: the actions to perform and any conflicts.
#[derive(Debug, Clone)]
pub struct RestorePlan {
    /// Actions that will be performed, in execution order.
    pub actions: Vec<RestoreAction>,
    /// Conflicts that require user confirmation.
    pub conflicts: Vec<Conflict>,
    /// Warnings about entries that cannot be fully restored.
    pub warnings: Vec<String>,
}

impl RestorePlan {
    /// Whether this plan has any conflicts requiring confirmation.
    pub fn has_conflicts(&self) -> bool {
        !self.conflicts.is_empty()
    }

    /// Number of actions in the plan.
    pub fn action_count(&self) -> usize {
        self.actions.len()
    }
}

/// Plan a restore by comparing the target snapshot with the current
/// filesystem state.
///
/// This is a pure function over pre-collected entry lists and does not
/// touch the filesystem. The returned plan lists every action needed and
/// any conflicts that require confirmation.
pub fn plan_restore(snapshot: &[TreeEntry], current: &[TreeEntry]) -> RestorePlan {
    let snap_map: BTreeMap<&PathBuf, &TreeEntry> = snapshot.iter().map(|e| (&e.path, e)).collect();
    let current_map: BTreeMap<&PathBuf, &TreeEntry> =
        current.iter().map(|e| (&e.path, e)).collect();

    let mut actions = Vec::new();
    let mut conflicts = Vec::new();
    let mut warnings = Vec::new();

    // Entries in the snapshot: restore or create.
    for (path, snap_entry) in &snap_map {
        match current_map.get(path) {
            None => {
                // Path doesn't exist now — need to create it.
                match snap_entry.meta.kind {
                    EntryKind::File => {
                        if let Some(ref hash) = snap_entry.meta.hash {
                            actions.push(RestoreAction::WriteFile {
                                path: (*path).clone(),
                                hash: hash.clone(),
                                readonly: snap_entry.meta.readonly,
                                mtime: snap_entry.meta.mtime,
                            });
                        } else {
                            warnings.push(format!(
                                "file has no content hash, cannot restore: {}",
                                path.display()
                            ));
                        }
                    }
                    EntryKind::Directory => {
                        actions.push(RestoreAction::CreateDir {
                            path: (*path).clone(),
                            readonly: snap_entry.meta.readonly,
                            mtime: snap_entry.meta.mtime,
                        });
                    }
                    EntryKind::Symlink => {
                        if let Some(ref target) = snap_entry.meta.target {
                            actions.push(RestoreAction::CreateSymlink {
                                path: (*path).clone(),
                                target: target.clone(),
                            });
                        }
                    }
                    EntryKind::Other => {
                        // Other entry types are not restored in this version.
                    }
                }
            }
            Some(curr_entry) => {
                // Path exists — check if it needs updating.
                if snap_entry.meta.kind != curr_entry.meta.kind {
                    // Kind changed (e.g., file → directory): conflict.
                    // We need to delete the current entry and create the
                    // snapshot's version.
                    conflicts.push(Conflict::Modified {
                        path: (*path).clone(),
                    });
                    actions.push(RestoreAction::Delete {
                        path: (*path).clone(),
                    });
                    match snap_entry.meta.kind {
                        EntryKind::File => {
                            if let Some(ref hash) = snap_entry.meta.hash {
                                actions.push(RestoreAction::WriteFile {
                                    path: (*path).clone(),
                                    hash: hash.clone(),
                                    readonly: snap_entry.meta.readonly,
                                    mtime: snap_entry.meta.mtime,
                                });
                            }
                        }
                        EntryKind::Directory => {
                            actions.push(RestoreAction::CreateDir {
                                path: (*path).clone(),
                                readonly: snap_entry.meta.readonly,
                                mtime: snap_entry.meta.mtime,
                            });
                        }
                        EntryKind::Symlink => {
                            if let Some(ref target) = snap_entry.meta.target {
                                actions.push(RestoreAction::CreateSymlink {
                                    path: (*path).clone(),
                                    target: target.clone(),
                                });
                            }
                        }
                        EntryKind::Other => {
                            // Other entry types are not restored.
                        }
                    }
                } else if snap_entry.meta.kind == EntryKind::File {
                    // Compare hashes for files.
                    let snap_hash = snap_entry.meta.hash.as_deref().unwrap_or("");
                    let curr_hash = curr_entry.meta.hash.as_deref().unwrap_or("");
                    if snap_hash != curr_hash {
                        // File content was modified — this is a conflict
                        // because restoring would overwrite the user's changes.
                        conflicts.push(Conflict::Modified {
                            path: (*path).clone(),
                        });
                        // Still add the write action — it will be performed
                        // after confirmation.
                        if let Some(ref hash) = snap_entry.meta.hash {
                            actions.push(RestoreAction::WriteFile {
                                path: (*path).clone(),
                                hash: hash.clone(),
                                readonly: snap_entry.meta.readonly,
                                mtime: snap_entry.meta.mtime,
                            });
                        }
                    } else if snap_entry.meta.readonly != curr_entry.meta.readonly
                        || snap_entry.meta.mtime != curr_entry.meta.mtime
                    {
                        // Content is the same but metadata (permissions or
                        // mtime) differs. This is not a conflict — restoring
                        // metadata does not destroy user data.
                        if let Some(ref hash) = snap_entry.meta.hash {
                            actions.push(RestoreAction::WriteFile {
                                path: (*path).clone(),
                                hash: hash.clone(),
                                readonly: snap_entry.meta.readonly,
                                mtime: snap_entry.meta.mtime,
                            });
                        }
                    }
                    // If hashes and metadata match, no action needed.
                } else if snap_entry.meta.kind == EntryKind::Symlink {
                    // Compare symlink targets.
                    let snap_target = snap_entry.meta.target.as_deref();
                    let curr_target = curr_entry.meta.target.as_deref();
                    if snap_target != curr_target {
                        // Symlink target changed — conflict.
                        conflicts.push(Conflict::Modified {
                            path: (*path).clone(),
                        });
                        if let Some(ref target) = snap_entry.meta.target {
                            // Delete the old symlink, then create the new one.
                            actions.push(RestoreAction::Delete {
                                path: (*path).clone(),
                            });
                            actions.push(RestoreAction::CreateSymlink {
                                path: (*path).clone(),
                                target: target.clone(),
                            });
                        }
                    }
                    // If targets match, no action needed.
                }
                // Directories with matching kind: no action needed.
            }
        }
    }

    // Entries in current but not in snapshot: delete (conflict).
    for path in current_map.keys() {
        if !snap_map.contains_key(path) {
            conflicts.push(Conflict::Unexpected {
                path: (*path).clone(),
            });
            actions.push(RestoreAction::Delete {
                path: (*path).clone(),
            });
        }
    }

    // Sort actions for deterministic execution:
    // 1. Delete entries that are being replaced (kind changes) — must happen
    //    before creating the new version at the same path.
    // 2. Create directories (parents before children)
    // 3. Write files and create symlinks
    // 4. Delete unexpected entries (not in snapshot) — last, so we don't
    //    delete a dir then try to create inside it.
    //
    // We distinguish "replace" deletes from "unexpected" deletes by checking
    // whether the path also has a Create/Write/Symlink action in the plan.
    let create_paths: std::collections::HashSet<PathBuf> = actions
        .iter()
        .filter_map(|a| match a {
            RestoreAction::CreateDir { path, .. }
            | RestoreAction::WriteFile { path, .. }
            | RestoreAction::CreateSymlink { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();

    actions.sort_by_key(|a| match a {
        // Replace deletes (same path has a create/write/symlink) go first.
        RestoreAction::Delete { path } if create_paths.contains(path) => (0, path.clone()),
        RestoreAction::CreateDir { path, .. } => (1, path.clone()),
        RestoreAction::WriteFile { path, .. } => (2, path.clone()),
        RestoreAction::CreateSymlink { path, .. } => (2, path.clone()),
        // Unexpected deletes go last.
        RestoreAction::Delete { path } => (3, path.clone()),
    });

    RestorePlan {
        actions,
        conflicts,
        warnings,
    }
}

/// The outcome of a restore operation.
#[derive(Debug, Clone)]
pub struct RestoreResult {
    /// Number of files written.
    pub files_written: usize,
    /// Number of directories created.
    pub dirs_created: usize,
    /// Number of symlinks created.
    pub symlinks_created: usize,
    /// Number of files/directories deleted.
    pub deleted: usize,
    /// Whether the post-restore verification passed.
    pub verified: bool,
    /// Any non-fatal warnings during restore.
    pub warnings: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{EntryKind, EntryMeta};
    use std::path::PathBuf;

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

    fn dir_entry(path: &str) -> TreeEntry {
        TreeEntry {
            path: PathBuf::from(path),
            meta: EntryMeta {
                kind: EntryKind::Directory,
                size: 0,
                readonly: false,
                mtime: None,
                hash: None,
                target: None,
            },
        }
    }

    fn symlink_entry(path: &str, target: &str) -> TreeEntry {
        TreeEntry {
            path: PathBuf::from(path),
            meta: EntryMeta {
                kind: EntryKind::Symlink,
                size: 0,
                readonly: false,
                mtime: None,
                hash: None,
                target: Some(PathBuf::from(target)),
            },
        }
    }

    #[test]
    fn is_safe_relative_path_accepts_normal() {
        assert!(is_safe_relative_path(&PathBuf::from("a.txt")));
        assert!(is_safe_relative_path(&PathBuf::from("src/main.rs")));
        assert!(is_safe_relative_path(&PathBuf::from("a/b/c/d.txt")));
        assert!(is_safe_relative_path(&PathBuf::from("./a.txt")));
    }

    #[test]
    fn is_safe_relative_path_rejects_traversal() {
        assert!(!is_safe_relative_path(&PathBuf::from("../a.txt")));
        assert!(!is_safe_relative_path(&PathBuf::from("a/../../b.txt")));
        assert!(!is_safe_relative_path(&PathBuf::from("/etc/passwd")));
        assert!(!is_safe_relative_path(&PathBuf::from("../")));
    }

    #[test]
    fn plan_restore_warns_on_file_without_hash() {
        let snapshot = vec![TreeEntry {
            path: PathBuf::from("a.txt"),
            meta: EntryMeta {
                kind: EntryKind::File,
                size: 10,
                readonly: false,
                mtime: None,
                hash: None,
                target: None,
            },
        }];
        let current = vec![];
        let plan = plan_restore(&snapshot, &current);
        assert!(!plan.warnings.is_empty(), "should warn about missing hash");
        assert!(plan.actions.is_empty(), "should not produce an action");
    }

    #[test]
    fn plan_restore_no_changes() {
        let snapshot = vec![file_entry("a.txt", Some("hash1"))];
        let current = vec![file_entry("a.txt", Some("hash1"))];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.actions.is_empty());
        assert!(plan.conflicts.is_empty());
    }

    #[test]
    fn plan_restore_modified_file_is_conflict() {
        let snapshot = vec![file_entry("a.txt", Some("hash1"))];
        let current = vec![file_entry("a.txt", Some("hash2"))];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.has_conflicts());
        assert_eq!(plan.conflicts.len(), 1);
        assert!(matches!(plan.conflicts[0], Conflict::Modified { .. }));
        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(plan.actions[0], RestoreAction::WriteFile { .. }));
    }

    #[test]
    fn plan_restore_unexpected_file_is_conflict() {
        let snapshot = vec![file_entry("a.txt", Some("hash1"))];
        let current = vec![
            file_entry("a.txt", Some("hash1")),
            file_entry("b.txt", Some("hash2")),
        ];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.has_conflicts());
        assert_eq!(plan.conflicts.len(), 1);
        assert!(matches!(plan.conflicts[0], Conflict::Unexpected { .. }));
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::Delete { .. }))
        );
    }

    #[test]
    fn plan_restore_deleted_file_needs_write() {
        let snapshot = vec![file_entry("a.txt", Some("hash1"))];
        let current = vec![];
        let plan = plan_restore(&snapshot, &current);
        assert!(!plan.has_conflicts());
        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(plan.actions[0], RestoreAction::WriteFile { .. }));
    }

    #[test]
    fn plan_restore_missing_directory_needs_create() {
        let snapshot = vec![dir_entry("subdir")];
        let current = vec![];
        let plan = plan_restore(&snapshot, &current);
        assert!(!plan.has_conflicts());
        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(plan.actions[0], RestoreAction::CreateDir { .. }));
    }

    #[test]
    fn plan_restore_kind_change_is_conflict() {
        let snapshot = vec![file_entry("a.txt", Some("hash1"))];
        let current = vec![dir_entry("a.txt")];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.has_conflicts());
        assert_eq!(plan.conflicts.len(), 1);
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::Delete { .. })),
            "should delete the old-kind entry"
        );
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::WriteFile { .. })),
            "should write the snapshot's version"
        );
        let delete_idx = plan
            .actions
            .iter()
            .position(|a| matches!(a, RestoreAction::Delete { .. }))
            .unwrap();
        let write_idx = plan
            .actions
            .iter()
            .position(|a| matches!(a, RestoreAction::WriteFile { .. }))
            .unwrap();
        assert!(
            delete_idx < write_idx,
            "delete must come before write for kind changes"
        );
    }

    #[test]
    fn plan_restore_missing_symlink_needs_create() {
        let snapshot = vec![symlink_entry("link", "target.txt")];
        let current = vec![];
        let plan = plan_restore(&snapshot, &current);
        assert!(!plan.has_conflicts());
        assert_eq!(plan.actions.len(), 1);
        assert!(matches!(
            plan.actions[0],
            RestoreAction::CreateSymlink { .. }
        ));
    }

    #[test]
    fn plan_restore_matching_symlink_no_action() {
        let snapshot = vec![symlink_entry("link", "target.txt")];
        let current = vec![symlink_entry("link", "target.txt")];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.actions.is_empty());
        assert!(!plan.has_conflicts());
    }

    #[test]
    fn plan_restore_changed_symlink_target_is_conflict() {
        let snapshot = vec![symlink_entry("link", "old.txt")];
        let current = vec![symlink_entry("link", "new.txt")];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.has_conflicts());
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::Delete { .. }))
        );
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::CreateSymlink { .. }))
        );
    }

    #[test]
    fn plan_restore_symlink_to_file_kind_change() {
        let snapshot = vec![symlink_entry("link", "target.txt")];
        let current = vec![file_entry("link", Some("hash1"))];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.has_conflicts());
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::Delete { .. }))
        );
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::CreateSymlink { .. }))
        );
    }

    #[test]
    fn plan_restore_combined_scenario() {
        let snapshot = vec![
            file_entry("keep.txt", Some("hash1")),
            file_entry("modify.txt", Some("hash2")),
            dir_entry("dir"),
        ];
        let current = vec![
            file_entry("keep.txt", Some("hash1")),
            file_entry("modify.txt", Some("hash3")),
            file_entry("new.txt", Some("hash4")),
        ];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.has_conflicts());
        assert!(plan.conflicts.len() >= 2);
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::WriteFile { .. }))
        );
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::Delete { .. }))
        );
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::CreateDir { .. }))
        );
    }

    #[test]
    fn plan_restore_actions_sorted_dirs_first() {
        let snapshot = vec![file_entry("a/b.txt", Some("hash1")), dir_entry("a")];
        let current = vec![];
        let plan = plan_restore(&snapshot, &current);
        let create_dir_idx = plan
            .actions
            .iter()
            .position(|a| matches!(a, RestoreAction::CreateDir { .. }))
            .unwrap();
        let write_idx = plan
            .actions
            .iter()
            .position(|a| matches!(a, RestoreAction::WriteFile { .. }))
            .unwrap();
        assert!(
            create_dir_idx < write_idx,
            "dirs should be created before files inside them"
        );
    }
}
