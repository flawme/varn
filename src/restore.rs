//! Restore engine: restoring a known state.
//!
//! The restore process follows a strict safety model:
//!
//! 1. **Plan** — compare the target snapshot with the current filesystem
//!    state and identify every action needed, plus any conflicts.
//! 2. **Confirm** — if conflicts exist, require explicit user confirmation
//!    (or the `--yes` flag).
//! 3. **Execute** — perform the actions: restore file contents from the
//!    object store, delete unexpected files, recreate directories.
//! 4. **Verify** — re-scan the filesystem and confirm it matches the
//!    snapshot.
//!
//! Restoration is treated as a potentially destructive operation. Files
//! that exist now but not in the checkpoint are deleted. Files that changed
//! since the checkpoint are overwritten. These actions are never performed
//! silently.

use crate::error::{Result, VarnError};
use crate::filesystem::{EntryKind, TreeEntry};
use crate::storage::ObjectStore;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

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
    WriteFile { path: PathBuf, hash: String },
    /// Create a directory.
    CreateDir { path: PathBuf },
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
                            });
                        }
                    }
                    EntryKind::Directory => {
                        actions.push(RestoreAction::CreateDir {
                            path: (*path).clone(),
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
                                });
                            }
                        }
                        EntryKind::Directory => {
                            actions.push(RestoreAction::CreateDir {
                                path: (*path).clone(),
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
                        // File was modified — this is a conflict because
                        // restoring would overwrite the user's changes.
                        conflicts.push(Conflict::Modified {
                            path: (*path).clone(),
                        });
                        // Still add the write action — it will be performed
                        // after confirmation.
                        if let Some(ref hash) = snap_entry.meta.hash {
                            actions.push(RestoreAction::WriteFile {
                                path: (*path).clone(),
                                hash: hash.clone(),
                            });
                        }
                    }
                    // If hashes match, no action needed.
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
            RestoreAction::CreateDir { path }
            | RestoreAction::WriteFile { path, .. }
            | RestoreAction::CreateSymlink { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect();

    actions.sort_by_key(|a| match a {
        // Replace deletes (same path has a create/write/symlink) go first.
        RestoreAction::Delete { path } if create_paths.contains(path) => (0, path.clone()),
        RestoreAction::CreateDir { path } => (1, path.clone()),
        RestoreAction::WriteFile { path, .. } => (2, path.clone()),
        RestoreAction::CreateSymlink { path, .. } => (2, path.clone()),
        // Unexpected deletes go last.
        RestoreAction::Delete { path } => (3, path.clone()),
    });

    RestorePlan { actions, conflicts }
}

/// Execute a restore plan against the filesystem.
///
/// This performs the actual file operations: creating directories, writing
/// file contents from the object store, and deleting unexpected files.
///
/// # Safety
///
/// This function performs destructive filesystem operations. The caller is
/// responsible for confirming any conflicts via the plan before calling
/// this function.
pub fn execute_restore(
    plan: &RestorePlan,
    root: &Path,
    store: &ObjectStore,
) -> Result<RestoreResult> {
    let mut files_written = 0;
    let mut dirs_created = 0;
    let mut symlinks_created = 0;
    let mut deleted = 0;
    let mut warnings = Vec::new();

    for action in &plan.actions {
        match action {
            RestoreAction::CreateDir { path } => {
                let full = root.join(path);
                fs::create_dir_all(&full)?;
                dirs_created += 1;
            }
            RestoreAction::WriteFile { path, hash } => {
                let full = root.join(path);
                // Ensure parent directory exists.
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent)?;
                }
                // Read content from the object store.
                let content = store.read_content(hash).map_err(|e| {
                    VarnError::Other(format!(
                        "cannot retrieve content for {} (hash {}): {e}",
                        path.display(),
                        hash
                    ))
                })?;
                fs::write(&full, &content)?;
                files_written += 1;
            }
            RestoreAction::CreateSymlink { path, target } => {
                let full = root.join(path);
                // Ensure parent directory exists.
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent)?;
                }
                // If something already exists at this path, remove it first.
                // (This can happen for replace-delete + create sequences.)
                if full.exists() || fs::symlink_metadata(&full).is_ok() {
                    let meta = fs::symlink_metadata(&full)?;
                    if meta.is_dir() {
                        fs::remove_dir_all(&full)?;
                    } else {
                        fs::remove_file(&full)?;
                    }
                }
                crate::platform::create_symlink(target, &full)?;
                symlinks_created += 1;
            }
            RestoreAction::Delete { path } => {
                let full = root.join(path);
                let meta = match fs::symlink_metadata(&full) {
                    Ok(m) => m,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // File already gone — not an error.
                        warnings.push(format!("file already deleted: {}", path.display()));
                        continue;
                    }
                    Err(e) => return Err(VarnError::Io(e)),
                };
                if meta.is_dir() {
                    fs::remove_dir_all(&full)?;
                } else {
                    fs::remove_file(&full)?;
                }
                deleted += 1;
            }
        }
    }

    Ok(RestoreResult {
        files_written,
        dirs_created,
        symlinks_created,
        deleted,
        verified: false,
        warnings,
    })
}

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
                // For directories, just kind matching is sufficient.
            }
        }
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filesystem::{EntryKind, EntryMeta};
    use crate::storage::Repo;
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
        // Should still have a write action to restore the file.
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
        // Should have a delete action.
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
        // Should have both a delete (old kind) and a write (new kind).
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
        // Delete must come before the write for the same path.
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
    fn plan_restore_actions_sorted_dirs_first() {
        let snapshot = vec![
            dir_entry("d"),
            file_entry("a.txt", Some("hash1")),
            dir_entry("b"),
        ];
        let current = vec![];
        let plan = plan_restore(&snapshot, &current);
        // Directories should come before files.
        let kinds: Vec<_> = plan
            .actions
            .iter()
            .map(|a| match a {
                RestoreAction::CreateDir { .. } => 0,
                RestoreAction::WriteFile { .. } => 1,
                RestoreAction::CreateSymlink { .. } => 1,
                RestoreAction::Delete { .. } => 2,
            })
            .collect();
        let mut sorted = kinds.clone();
        sorted.sort();
        assert_eq!(kinds, sorted);
    }

    #[test]
    fn plan_restore_combined_scenario() {
        let snapshot = vec![
            file_entry("keep.txt", Some("h1")),
            file_entry("restore.txt", Some("h2")),
            dir_entry("dir1"),
        ];
        let current = vec![
            file_entry("keep.txt", Some("h1")),
            file_entry("restore.txt", Some("h3")), // modified
            file_entry("unexpected.txt", Some("h4")), // unexpected
            dir_entry("dir1"),
        ];
        let plan = plan_restore(&snapshot, &current);
        // Conflicts: restore.txt modified, unexpected.txt unexpected.
        assert_eq!(plan.conflicts.len(), 2);
        // Actions: write restore.txt, delete unexpected.txt.
        assert_eq!(plan.actions.len(), 2);
    }

    #[test]
    fn plan_restore_missing_symlink_needs_create() {
        let snapshot = vec![symlink_entry("link.txt", "target.txt")];
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
        let snapshot = vec![symlink_entry("link.txt", "target.txt")];
        let current = vec![symlink_entry("link.txt", "target.txt")];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.actions.is_empty());
        assert!(plan.conflicts.is_empty());
    }

    #[test]
    fn plan_restore_changed_symlink_target_is_conflict() {
        let snapshot = vec![symlink_entry("link.txt", "target_a.txt")];
        let current = vec![symlink_entry("link.txt", "target_b.txt")];
        let plan = plan_restore(&snapshot, &current);
        assert!(plan.has_conflicts());
        assert_eq!(plan.conflicts.len(), 1);
        assert!(matches!(plan.conflicts[0], Conflict::Modified { .. }));
        // Should have a delete + create symlink sequence.
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::Delete { .. })),
            "should delete old symlink"
        );
        assert!(
            plan.actions
                .iter()
                .any(|a| matches!(a, RestoreAction::CreateSymlink { .. })),
            "should create new symlink"
        );
        // Delete must come before create.
        let delete_idx = plan
            .actions
            .iter()
            .position(|a| matches!(a, RestoreAction::Delete { .. }))
            .unwrap();
        let create_idx = plan
            .actions
            .iter()
            .position(|a| matches!(a, RestoreAction::CreateSymlink { .. }))
            .unwrap();
        assert!(delete_idx < create_idx, "delete must come before create");
    }

    #[test]
    fn plan_restore_symlink_to_file_kind_change() {
        let snapshot = vec![symlink_entry("link.txt", "target.txt")];
        let current = vec![file_entry("link.txt", Some("hash1"))];
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
    fn execute_restore_creates_symlink() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateSymlink {
                path: PathBuf::from("link.txt"),
                target: PathBuf::from("target.txt"),
            }],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.symlinks_created, 1);

        let link_path = tmp.path().join("link.txt");
        assert!(link_path.is_symlink());
        assert_eq!(
            fs::read_link(&link_path).unwrap(),
            PathBuf::from("target.txt")
        );
    }

    #[test]
    fn execute_restore_replaces_symlink_with_new_target() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Create an existing symlink.
        crate::platform::create_symlink(
            &PathBuf::from("old_target.txt"),
            &tmp.path().join("link.txt"),
        )
        .unwrap();

        // Plan: delete old + create new.
        let plan = RestorePlan {
            actions: vec![
                RestoreAction::Delete {
                    path: PathBuf::from("link.txt"),
                },
                RestoreAction::CreateSymlink {
                    path: PathBuf::from("link.txt"),
                    target: PathBuf::from("new_target.txt"),
                },
            ],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.symlinks_created, 1);

        let link_path = tmp.path().join("link.txt");
        assert!(link_path.is_symlink());
        assert_eq!(
            fs::read_link(&link_path).unwrap(),
            PathBuf::from("new_target.txt")
        );
    }

    #[test]
    fn execute_restore_creates_nested_symlink() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateSymlink {
                path: PathBuf::from("a/b/link.txt"),
                target: PathBuf::from("target.txt"),
            }],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.symlinks_created, 1);
        assert!(tmp.path().join("a/b/link.txt").is_symlink());
    }

    #[test]
    fn execute_restore_writes_file_from_object_store() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Store content in the object store.
        let hash = "abcdef123456";
        store.store_content(hash, b"restored content").unwrap();

        // Plan: write a file.
        let plan = RestorePlan {
            actions: vec![RestoreAction::WriteFile {
                path: PathBuf::from("a.txt"),
                hash: hash.to_string(),
            }],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.files_written, 1);
        assert_eq!(
            fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
            "restored content"
        );
    }

    #[test]
    fn execute_restore_creates_directories() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        let plan = RestorePlan {
            actions: vec![
                RestoreAction::CreateDir {
                    path: PathBuf::from("a"),
                },
                RestoreAction::CreateDir {
                    path: PathBuf::from("a/b"),
                },
            ],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.dirs_created, 2);
        assert!(tmp.path().join("a/b").is_dir());
    }

    #[test]
    fn execute_restore_deletes_files() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Create a file to be deleted.
        fs::write(tmp.path().join("to_delete.txt"), b"delete me").unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::Delete {
                path: PathBuf::from("to_delete.txt"),
            }],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.deleted, 1);
        assert!(!tmp.path().join("to_delete.txt").exists());
    }

    #[test]
    fn execute_restore_deletes_directories() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        fs::create_dir_all(tmp.path().join("to_delete")).unwrap();
        fs::write(tmp.path().join("to_delete/inner.txt"), b"x").unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::Delete {
                path: PathBuf::from("to_delete"),
            }],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.deleted, 1);
        assert!(!tmp.path().join("to_delete").exists());
    }

    #[test]
    fn execute_restore_creates_parent_dirs_for_files() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        let hash = "abcdef123456";
        store.store_content(hash, b"nested content").unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::WriteFile {
                path: PathBuf::from("a/b/c/file.txt"),
                hash: hash.to_string(),
            }],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.files_written, 1);
        assert!(tmp.path().join("a/b/c/file.txt").is_file());
    }

    #[test]
    fn execute_restore_delete_missing_file_is_warning() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        let plan = RestorePlan {
            actions: vec![RestoreAction::Delete {
                path: PathBuf::from("nonexistent.txt"),
            }],
            conflicts: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.deleted, 0);
        assert_eq!(result.warnings.len(), 1);
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

    #[test]
    fn full_restore_round_trip() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Set up initial state.
        fs::write(tmp.path().join("a.txt"), b"content a").unwrap();
        fs::create_dir_all(tmp.path().join("sub")).unwrap();
        fs::write(tmp.path().join("sub/b.txt"), b"content b").unwrap();

        // Scan and store content.
        let scanner = crate::filesystem::Scanner::new(tmp.path());
        let scan = scanner.scan().unwrap();
        for entry in &scan.entries {
            if let Some(ref hash) = entry.meta.hash {
                let content = fs::read(tmp.path().join(&entry.path)).unwrap();
                store.store_content(hash, &content).unwrap();
            }
        }
        let snapshot = scan.entries.clone();

        // Modify the filesystem.
        fs::write(tmp.path().join("a.txt"), b"modified").unwrap();
        fs::remove_file(tmp.path().join("sub/b.txt")).unwrap();
        fs::write(tmp.path().join("new.txt"), b"new").unwrap();

        // Plan the restore.
        let current_scan = scanner.scan().unwrap();
        let plan = plan_restore(&snapshot, &current_scan.entries);

        // Execute the restore.
        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert!(result.files_written > 0 || result.deleted > 0);

        // Verify the restore.
        assert!(
            verify_restore(tmp.path(), &snapshot),
            "filesystem should match snapshot after restore"
        );
    }
}
