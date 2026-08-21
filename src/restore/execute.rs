//! Restore execution: performing the planned filesystem operations.
//!
//! The execute phase performs the actual file operations: creating
//! directories, writing file contents from the object store, creating
//! symlinks, and deleting unexpected files.
//!
//! # Safety
//!
//! This function performs destructive filesystem operations. The caller is
//! responsible for confirming any conflicts via the plan before calling
//! [`execute_restore`].

use crate::error::{Result, VarnError};
use crate::restore::plan::{RestoreAction, RestorePlan, RestoreResult, is_safe_relative_path};
use crate::storage::ObjectStore;
use std::fs;
use std::path::{Path, PathBuf};

/// Check that no ancestor of `full` (from `root` up to but not including
/// the final component) is a symlink. A symlink in the leading path could
/// cause a write to escape the managed root — the same class of bug as
/// CVE-2026-71556 (go-git) and GHSA-9qw7-j9xw-fv9c (isomorphic-git).
fn has_symlink_in_leading_path(root: &Path, full: &Path) -> bool {
    // Walk from root toward the target, checking each intermediate component.
    // We skip the final component because the caller handles replacement of
    // the target separately.
    let mut current: PathBuf = root.to_path_buf();
    for component in full.strip_prefix(root).unwrap_or(full).components() {
        current = current.join(component);
        // Stop before checking the final component.
        if current == full {
            break;
        }
        if fs::symlink_metadata(&current)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Apply readonly and mtime metadata to a path after it has been written.
fn apply_metadata(path: &Path, readonly: bool, mtime: Option<i64>) {
    if readonly {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_mode(perms.mode() & !0o200);
                let _ = fs::set_permissions(path, perms);
            }
        }
        #[cfg(not(unix))]
        {
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_readonly(true);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
    if let Some(ts) = mtime {
        let _ = filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(ts, 0));
    }
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

    // Validate all paths in the plan before touching the filesystem.
    for action in &plan.actions {
        let path = match action {
            RestoreAction::CreateDir { path, .. }
            | RestoreAction::WriteFile { path, .. }
            | RestoreAction::CreateSymlink { path, .. }
            | RestoreAction::Delete { path } => path,
        };
        if !is_safe_relative_path(path) {
            return Err(VarnError::InvalidPath(format!(
                "unsafe path in restore plan (could escape root): {}",
                path.display()
            )));
        }
    }

    // Pre-flight check: verify all objects exist before modifying anything.
    // This prevents partial restores where some files are written and then
    // a missing object aborts the rest.
    for action in &plan.actions {
        if let RestoreAction::WriteFile { path, hash, .. } = action {
            if !store.exists(hash) {
                return Err(VarnError::Other(format!(
                    "missing object for {} (hash {}): cannot restore — no changes were made",
                    path.display(),
                    hash
                )));
            }
        }
    }

    for action in &plan.actions {
        match action {
            RestoreAction::CreateDir {
                path,
                readonly,
                mtime,
            } => {
                let full = root.join(path);
                // Check for symlink in leading path to prevent escape.
                if has_symlink_in_leading_path(root, &full) {
                    return Err(VarnError::InvalidPath(format!(
                        "refusing to create directory through a symlink in the path (could escape root): {}",
                        path.display()
                    )));
                }
                fs::create_dir_all(&full)?;
                apply_metadata(&full, *readonly, *mtime);
                dirs_created += 1;
            }
            RestoreAction::WriteFile {
                path,
                hash,
                readonly,
                mtime,
            } => {
                let full = root.join(path);
                // Check for symlink in leading path to prevent escape.
                if has_symlink_in_leading_path(root, &full) {
                    return Err(VarnError::InvalidPath(format!(
                        "refusing to write file through a symlink in the path (could escape root): {}",
                        path.display()
                    )));
                }
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
                // Verify the content hash matches before writing.
                // This catches corrupted or tampered objects (bit rot, disk
                // errors, manual tampering) before they overwrite user data.
                let actual_hash = crate::filesystem::hash_bytes(&content);
                if actual_hash != *hash {
                    return Err(VarnError::Other(format!(
                        "object content hash mismatch for {} (expected {}, got {}): \
                         object is corrupted — no changes were made to this file",
                        path.display(),
                        hash,
                        actual_hash
                    )));
                }
                fs::write(&full, &content)?;
                apply_metadata(&full, *readonly, *mtime);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::restore::plan::{RestoreAction, RestorePlan};
    use crate::storage::Repo;
    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn execute_restore_rejects_unsafe_path() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        let plan = RestorePlan {
            actions: vec![RestoreAction::WriteFile {
                path: PathBuf::from("../../../etc/passwd"),
                hash: "abcdef123456".to_string(),
                readonly: false,
                mtime: None,
            }],
            conflicts: vec![],
            warnings: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store);
        assert!(result.is_err(), "should reject unsafe path");
        let err = result.unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));
    }

    #[test]
    fn execute_restore_writes_file_from_object_store() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Store content in the object store with its real hash.
        let content = b"restored content";
        let hash = crate::filesystem::hash_bytes(content);
        store.store_content(&hash, content).unwrap();

        // Plan: write a file.
        let plan = RestorePlan {
            actions: vec![RestoreAction::WriteFile {
                path: PathBuf::from("a.txt"),
                hash,
                readonly: false,
                mtime: None,
            }],
            conflicts: vec![],
            warnings: vec![],
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
                    readonly: false,
                    mtime: None,
                },
                RestoreAction::CreateDir {
                    path: PathBuf::from("a/b"),
                    readonly: false,
                    mtime: None,
                },
            ],
            conflicts: vec![],
            warnings: vec![],
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

        fs::write(tmp.path().join("to_delete.txt"), b"x").unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::Delete {
                path: PathBuf::from("to_delete.txt"),
            }],
            conflicts: vec![],
            warnings: vec![],
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
            warnings: vec![],
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

        let content = b"nested content";
        let hash = crate::filesystem::hash_bytes(content);
        store.store_content(&hash, content).unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::WriteFile {
                path: PathBuf::from("a/b/c/file.txt"),
                hash,
                readonly: false,
                mtime: None,
            }],
            conflicts: vec![],
            warnings: vec![],
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
            warnings: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.deleted, 0);
        assert_eq!(result.warnings.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn execute_restore_creates_symlink() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        fs::write(tmp.path().join("target.txt"), b"target").unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateSymlink {
                path: PathBuf::from("link.txt"),
                target: tmp.path().join("target.txt"),
            }],
            conflicts: vec![],
            warnings: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.symlinks_created, 1);
        assert!(tmp.path().join("link.txt").is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn execute_restore_creates_nested_symlink() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        fs::write(tmp.path().join("target.txt"), b"target").unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateSymlink {
                path: PathBuf::from("sub/dir/link.txt"),
                target: tmp.path().join("target.txt"),
            }],
            conflicts: vec![],
            warnings: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.symlinks_created, 1);
        assert!(tmp.path().join("sub/dir/link.txt").is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn execute_restore_replaces_symlink_with_new_target() {
        use std::os::unix::fs::symlink;
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        fs::write(tmp.path().join("old_target.txt"), b"old").unwrap();
        fs::write(tmp.path().join("new_target.txt"), b"new").unwrap();
        symlink(
            tmp.path().join("old_target.txt"),
            tmp.path().join("link.txt"),
        )
        .unwrap();

        let plan = RestorePlan {
            actions: vec![
                RestoreAction::Delete {
                    path: PathBuf::from("link.txt"),
                },
                RestoreAction::CreateSymlink {
                    path: PathBuf::from("link.txt"),
                    target: tmp.path().join("new_target.txt"),
                },
            ],
            conflicts: vec![],
            warnings: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.symlinks_created, 1);
        assert_eq!(
            fs::read_link(tmp.path().join("link.txt")).unwrap(),
            tmp.path().join("new_target.txt")
        );
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
        let plan = crate::restore::plan_restore(&snapshot, &current_scan.entries);

        // Execute the restore.
        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert!(result.files_written > 0 || result.deleted > 0);

        // Verify the restore.
        assert!(
            crate::restore::verify_restore(tmp.path(), &snapshot),
            "filesystem should match snapshot after restore"
        );
    }
}
