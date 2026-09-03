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

/// Clear write-protection on a path so it can be overwritten or deleted.
///
/// Windows refuses to open a read-only file for writing and refuses to
/// delete one; Unix refuses to write into a read-only file. Restore must
/// clear the protection first, then re-apply the snapshot's attributes
/// afterwards (apply_metadata does that).
fn clear_write_protection(path: &Path) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if let Ok(meta) = fs::symlink_metadata(path) {
            let attrs = meta.file_attributes();
            const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
            const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
            const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
            if attrs & (FILE_ATTRIBUTE_READONLY | FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM)
                != 0
            {
                let _ = crate::platform::set_file_attributes(
                    path,
                    attrs
                        & !(FILE_ATTRIBUTE_READONLY
                            | FILE_ATTRIBUTE_HIDDEN
                            | FILE_ATTRIBUTE_SYSTEM),
                );
            }
        }
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::symlink_metadata(path) {
            let mode = meta.permissions().mode();
            // Owner write bit cleared, or not the owner: attempt to add
            // write permission for the owner; ignore errors (best-effort).
            if mode & 0o200 == 0 {
                if let Ok(meta) = fs::metadata(path) {
                    let mut perms = meta.permissions();
                    perms.set_mode(mode | 0o200);
                    let _ = fs::set_permissions(path, perms);
                }
            }
        }
    }
}

/// Remove one filesystem entry without following an NTFS junction.
///
/// `remove_dir_all` is appropriate for a real directory but is dangerous for
/// a junction: depending on the Windows API path, it can traverse into the
/// target tree. A junction itself is an empty reparse-point directory and
/// must be removed with `remove_dir`.
fn remove_existing_entry(path: &Path, meta: &fs::Metadata) -> std::io::Result<()> {
    if crate::platform::is_junction(path) {
        fs::remove_dir(path)
    } else if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Whether some parent component of `path` is scheduled to change kind
/// (directory replaced by file or vice versa) by the plan.
///
/// When the layout around a path changes during the restore, probing the
/// path's writability up front is meaningless — the probe would hit a
/// stale layout (e.g. ENOTDIR) and reject a restore that would succeed.
fn layout_will_change(plan: &RestorePlan, path: &Path) -> bool {
    plan.actions.iter().any(|a| match a {
        RestoreAction::Delete { path: p } => path.starts_with(p) || p.starts_with(path),
        _ => false,
    })
}

/// Probe whether a path can be written or deleted.
///
/// Returns `Err` with an actionable message when the path exists but is
/// locked by another process (Windows: sharing violation; Unix: permission
/// or mandatory lock). Used by the pre-flight check so a locked file aborts
/// the restore BEFORE any changes are made, instead of leaving a partial
/// tree behind.
fn probe_writable(path: &Path) -> std::result::Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    // Opening for write access detects sharing violations and permission
    // problems without modifying the file. Directories cannot be opened
    // this way; their deletability is checked via the parent.
    if path.is_dir() {
        return Ok(());
    }
    match fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => Ok(()),
        // NotADirectory means a parent path component is currently a file
        // but will be replaced by a directory earlier in the plan — not a
        // lock. Only genuine access/sharing problems block the restore.
        Err(e) if e.kind() == std::io::ErrorKind::NotADirectory => Ok(()),
        Err(e) => Err(format!(
            "{} is not writable (locked by another process or access denied): {e}",
            path.display()
        )),
    }
}

/// Captured metadata to apply to a restored entry, independent of platform.
///
/// Fields that do not apply to the current platform are `None` and ignored.
#[derive(Debug, Clone, Copy, Default)]
pub struct MetadataToApply<'a> {
    /// Whether the entry should be read-only (legacy fallback when no full
    /// mode was captured).
    pub readonly: bool,
    /// Modification time (unix seconds), if available.
    pub mtime: Option<i64>,
    /// Unix owner uid, if available.
    pub uid: Option<u32>,
    /// Unix owner gid, if available.
    pub gid: Option<u32>,
    /// Full Unix permission mode, if captured.
    pub mode: Option<u32>,
    /// macOS BSD file flags, if captured.
    pub flags: Option<u32>,
    /// Windows file attributes, if captured.
    pub attributes: Option<u32>,
    /// Windows security descriptor in SDDL form, if captured.
    pub acl: Option<&'a str>,
}

/// Apply captured metadata to a restored entry.
///
/// Every step is best-effort: a failure to apply one piece of metadata is
/// recorded as a warning and never fails the restore. Order matters —
/// permissions are applied before ownership (chown can clear setuid/setgid
/// bits), and read-only attributes are applied last so later steps can
/// still write.
fn apply_metadata(path: &Path, meta: &MetadataToApply, warnings: &mut Vec<String>) {
    let MetadataToApply {
        readonly,
        mtime,
        uid,
        gid,
        mode,
        flags,
        attributes,
        acl,
    } = *meta;

    // Full Unix mode (all Unix platforms). Applied when the snapshot has a
    // mode; falls back to the readonly bit otherwise (older snapshots).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(mode) = mode {
            if let Ok(m) = fs::metadata(path) {
                let mut perms = m.permissions();
                perms.set_mode(mode);
                if let Err(e) = fs::set_permissions(path, perms) {
                    warnings.push(format!(
                        "cannot restore mode {:o} on {}: {e}",
                        mode,
                        path.display()
                    ));
                }
            }
        } else if readonly {
            if let Ok(m) = fs::metadata(path) {
                let mut perms = m.permissions();
                perms.set_mode(perms.mode() & !0o200);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = mode;
    }

    // Unix ownership. Requires root for changing to a different user.
    #[cfg(unix)]
    {
        use std::os::unix::fs::chown;
        if uid.is_some() || gid.is_some() {
            if let Err(e) = chown(path, uid, gid) {
                warnings.push(format!(
                    "cannot restore ownership on {}: {e} (requires appropriate privileges)",
                    path.display()
                ));
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = (uid, gid);
    }

    // macOS BSD file flags. Best-effort: privileged flags (schg, uchg with
    // restricted bits) fail with EPERM for unprivileged users; borg's
    // practice is to warn and continue.
    #[cfg(target_os = "macos")]
    {
        if let Some(flags) = flags {
            match crate::platform::set_bsd_flags(path, flags) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    warnings.push(format!(
                        "cannot restore BSD flags {:#x} on {}: requires privileges",
                        flags,
                        path.display()
                    ));
                }
                Err(e) => {
                    warnings.push(format!(
                        "cannot restore BSD flags {:#x} on {}: {e}",
                        flags,
                        path.display()
                    ));
                }
            }
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = flags;
    }

    // Windows ACL / security descriptor. Applied before attributes because
    // a restrictive DACL could block the attribute change.
    #[cfg(windows)]
    {
        if let Some(sddl) = acl {
            if let Err(e) = crate::platform::set_security_descriptor(path, sddl) {
                warnings.push(format!(
                    "cannot restore security descriptor on {}: {}",
                    path.display(),
                    e
                ));
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = acl;
    }

    // Modification time. Applied BEFORE read-only protection: Windows
    // refuses SetFileTime on a read-only file, so setting the attribute
    // first would leave the mtime unrestored (and verification would
    // report a false failure). Failures are warnings, not silent.
    if let Some(ts) = mtime {
        if let Err(e) = filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(ts, 0)) {
            warnings.push(format!(
                "cannot restore mtime {} on {}: {}",
                ts,
                path.display(),
                e
            ));
        }
    }

    // Windows file attributes (READONLY, HIDDEN, SYSTEM, ARCHIVE, ...).
    // Applied LAST so protection does not block mtime/ACL steps above.
    #[cfg(windows)]
    {
        if let Some(attrs) = attributes {
            if let Err(e) = crate::platform::set_file_attributes(path, attrs) {
                warnings.push(format!(
                    "cannot restore file attributes {:#x} on {}: {}",
                    attrs,
                    path.display(),
                    e
                ));
            }
        } else if readonly {
            if let Ok(meta) = fs::metadata(path) {
                let mut perms = meta.permissions();
                perms.set_readonly(true);
                let _ = fs::set_permissions(path, perms);
            }
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (attributes, readonly);
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
            | RestoreAction::CreateJunction { path, .. }
            | RestoreAction::CreateHardLink { path, .. }
            | RestoreAction::ApplyDirMeta { path, .. }
            | RestoreAction::Delete { path } => path,
        };
        if !is_safe_relative_path(path) {
            return Err(VarnError::InvalidPath(format!(
                "unsafe path in restore plan (could escape root): {}",
                path.display()
            )));
        }
        // For hard links, also validate the target path.
        if let RestoreAction::CreateHardLink { target, .. } = action {
            if !is_safe_relative_path(target) {
                return Err(VarnError::InvalidPath(format!(
                    "unsafe hard link target in restore plan (could escape root): {}",
                    target.display()
                )));
            }
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

    // Pre-flight lock probe: every file that would be overwritten or
    // deleted must be writable NOW, or the restore aborts before touching
    // anything. Without this, a file locked by another process (editor,
    // indexer, antivirus) aborts mid-restore and leaves a partial tree.
    for action in &plan.actions {
        let target = match action {
            RestoreAction::WriteFile { path, .. } => Some(path),
            RestoreAction::Delete { path } => Some(path),
            _ => None,
        };
        if let Some(path) = target {
            let full = root.join(path);
            // Skip the probe when a parent path component is currently a
            // directory that the plan will replace with a file (or vice
            // versa): the layout changes before this action runs, so the
            // probe's result would be meaningless.
            if layout_will_change(plan, path) {
                continue;
            }
            // A write-protected file is not a lock: restore clears
            // protection before writing (that is part of its job). Clear
            // it before probing; only a genuine sharing violation or
            // access denial that survives this blocks the restore.
            if full.exists() && !full.is_dir() {
                clear_write_protection(&full);
            }
            if let Err(msg) = probe_writable(&full) {
                return Err(VarnError::Other(format!(
                    "pre-flight check failed: {msg} — no changes were made; \
                     close the program using the file and retry"
                )));
            }
        }
    }

    // Execute the actions. ApplyDirMeta actions (which sort last in the
    // plan) are deferred to a second pass below: they must run after every
    // child operation because child operations update the parent
    // directory's mtime. Within the pass they run in reverse plan order
    // (deepest first) for the same reason.
    let mut dir_meta_actions: Vec<&RestoreAction> = Vec::new();
    for action in &plan.actions {
        if let RestoreAction::ApplyDirMeta { .. } = action {
            dir_meta_actions.push(action);
            continue;
        }
        if std::env::var("VARN_TRACE").is_ok() {
            eprintln!("EXEC: {action:?}");
        }
        match action {
            RestoreAction::CreateDir {
                path,
                readonly,
                mtime,
                uid,
                gid,
                mode,
                flags,
                attributes,
                acl,
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
                apply_metadata(
                    &full,
                    &MetadataToApply {
                        readonly: *readonly,
                        mtime: *mtime,
                        uid: *uid,
                        gid: *gid,
                        mode: *mode,
                        flags: *flags,
                        attributes: *attributes,
                        acl: acl.as_deref(),
                    },
                    &mut warnings,
                );
                dirs_created += 1;
            }
            RestoreAction::WriteFile {
                path,
                hash,
                readonly,
                mtime,
                uid,
                gid,
                mode,
                flags,
                attributes,
                acl,
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
                // Clear write-protection first: Windows refuses to open a
                // read-only file for writing (os error 5), which made every
                // re-restore of a checkpoint containing read-only files fail.
                if full.exists() {
                    clear_write_protection(&full);
                }
                fs::write(&full, &content)?;
                apply_metadata(
                    &full,
                    &MetadataToApply {
                        readonly: *readonly,
                        mtime: *mtime,
                        uid: *uid,
                        gid: *gid,
                        mode: *mode,
                        flags: *flags,
                        attributes: *attributes,
                        acl: acl.as_deref(),
                    },
                    &mut warnings,
                );
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
                    clear_write_protection(&full);
                    let meta = fs::symlink_metadata(&full)?;
                    remove_existing_entry(&full, &meta)?;
                }
                crate::platform::create_symlink(target, &full)?;
                symlinks_created += 1;
            }
            RestoreAction::CreateJunction { path, target } => {
                let full = root.join(path);
                if has_symlink_in_leading_path(root, &full) {
                    return Err(VarnError::InvalidPath(format!(
                        "refusing to create junction through a symlink in the path (could escape root): {}",
                        path.display()
                    )));
                }
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent)?;
                }
                if full.exists() || fs::symlink_metadata(&full).is_ok() {
                    clear_write_protection(&full);
                    let meta = fs::symlink_metadata(&full)?;
                    remove_existing_entry(&full, &meta)?;
                }
                crate::platform::create_junction(target, &full)?;
                symlinks_created += 1;
            }
            RestoreAction::CreateHardLink { path, target } => {
                let full = root.join(path);
                let target_full = root.join(target);
                // Check for symlink in leading path to prevent escape.
                // Same class of bug as CVE-2026-71556 (go-git) and
                // GHSA-9qw7-j9xw-fv9c (isomorphic-git).
                if has_symlink_in_leading_path(root, &full) {
                    return Err(VarnError::InvalidPath(format!(
                        "refusing to create hard link through a symlink in the path (could escape root): {}",
                        path.display()
                    )));
                }
                // Also check the target path for symlinks.
                if has_symlink_in_leading_path(root, &target_full) {
                    return Err(VarnError::InvalidPath(format!(
                        "refusing to hard link to a target through a symlink in the path (could escape root): {}",
                        target.display()
                    )));
                }
                // Check that the target itself is not a symlink. The leading
                // path check skips the final component, but the target file
                // could itself be a symlink to an external location.
                // CVE-2026-32232 (ZeptoClaw) R3: hardlink alias bypass.
                if let Ok(meta) = fs::symlink_metadata(&target_full) {
                    if meta.file_type().is_symlink() {
                        return Err(VarnError::InvalidPath(format!(
                            "refusing to hard link to a symlink target (could alias external inode): {}",
                            target.display()
                        )));
                    }
                } else {
                    return Err(VarnError::Other(format!(
                        "hard link target does not exist: {}",
                        target.display()
                    )));
                }
                // Ensure parent directory exists.
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent)?;
                }
                // If something already exists at this path, remove it first.
                if full.exists() || fs::symlink_metadata(&full).is_ok() {
                    let meta = fs::symlink_metadata(&full)?;
                    remove_existing_entry(&full, &meta)?;
                }
                // Create a hard link to the primary file.
                // The target must have been restored first (sorting ensures
                // WriteFile actions come before CreateHardLink at the same
                // priority level, but cross-directory ordering may vary).
                fs::hard_link(&target_full, &full).map_err(|e| {
                    VarnError::Other(format!(
                        "cannot create hard link {} -> {}: {e}",
                        path.display(),
                        target.display()
                    ))
                })?;
                files_written += 1;
            }
            RestoreAction::Delete { path } => {
                let full = root.join(path);
                let meta = match fs::symlink_metadata(&full) {
                    Ok(m) => m,
                    // Already gone, or the parent directory is already gone
                    // (removed by an earlier Delete of a parent directory):
                    // both mean the entry is already deleted — not an error.
                    Err(e)
                        if e.kind() == std::io::ErrorKind::NotFound
                            || e.kind() == std::io::ErrorKind::NotADirectory =>
                    {
                        warnings.push(format!("file already deleted: {}", path.display()));
                        continue;
                    }
                    Err(e) => return Err(VarnError::Io(e)),
                };
                // Clear write-protection before deleting (Windows refuses
                // to delete read-only files).
                clear_write_protection(&full);
                remove_existing_entry(&full, &meta)?;
                deleted += 1;
            }
            RestoreAction::ApplyDirMeta { .. } => unreachable!("deferred above"),
        }
    }

    // Directory metadata pass: deepest first (plan sorted by depth
    // ascending; reverse visitation applies deepest last-planned first).
    for action in dir_meta_actions.into_iter().rev() {
        if std::env::var("VARN_TRACE").is_ok() {
            eprintln!("DIRMETA: {action:?}");
        }
        if let RestoreAction::ApplyDirMeta {
            path,
            readonly,
            mtime,
            uid,
            gid,
            mode,
            flags,
            attributes,
            acl,
        } = action
        {
            let full = root.join(path);
            if !full.is_dir() {
                return Err(VarnError::Other(format!(
                    "cannot apply directory metadata: {} does not exist or is not a directory",
                    path.display()
                )));
            }
            apply_metadata(
                &full,
                &MetadataToApply {
                    readonly: *readonly,
                    mtime: *mtime,
                    uid: *uid,
                    gid: *gid,
                    mode: *mode,
                    flags: *flags,
                    attributes: *attributes,
                    acl: acl.as_deref(),
                },
                &mut warnings,
            );
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
                uid: None,
                gid: None,
                mode: None,
                flags: None,
                attributes: None,
                acl: None,
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
                uid: None,
                gid: None,
                mode: None,
                flags: None,
                attributes: None,
                acl: None,
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
                    uid: None,
                    gid: None,
                    mode: None,
                    flags: None,
                    attributes: None,
                    acl: None,
                },
                RestoreAction::CreateDir {
                    path: PathBuf::from("a/b"),
                    readonly: false,
                    mtime: None,
                    uid: None,
                    gid: None,
                    mode: None,
                    flags: None,
                    attributes: None,
                    acl: None,
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
                uid: None,
                gid: None,
                mode: None,
                flags: None,
                attributes: None,
                acl: None,
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

    #[cfg(unix)]
    #[test]
    fn execute_restore_hard_link_refuses_symlink_in_path() {
        // CVE-2024-32002 pattern: a symlink in the leading path of a
        // CreateHardLink action could cause the link target to escape the
        // managed root. The execute function must refuse this.
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Create a symlink that points outside the root.
        let outside = TempDir::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("evil")).unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateHardLink {
                path: std::path::PathBuf::from("evil/payload.txt"),
                target: std::path::PathBuf::from("original.txt"),
            }],
            conflicts: vec![],
            warnings: vec![],
        };

        let err = execute_restore(&plan, tmp.path(), &store).unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn execute_restore_hard_link_refuses_symlink_in_target() {
        // The hard link target path must also be checked for symlinks.
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Create a symlink that points outside the root.
        let outside = TempDir::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("evil_target")).unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateHardLink {
                path: std::path::PathBuf::from("safe.txt"),
                target: std::path::PathBuf::from("evil_target/secret.txt"),
            }],
            conflicts: vec![],
            warnings: vec![],
        };

        let err = execute_restore(&plan, tmp.path(), &store).unwrap_err();
        assert!(matches!(err, VarnError::InvalidPath(_)));
    }

    #[cfg(unix)]
    #[test]
    fn execute_restore_hard_link_creates_link() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Create the primary file.
        fs::write(tmp.path().join("original.txt"), b"shared content").unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateHardLink {
                path: std::path::PathBuf::from("copy.txt"),
                target: std::path::PathBuf::from("original.txt"),
            }],
            conflicts: vec![],
            warnings: vec![],
        };

        let result = execute_restore(&plan, tmp.path(), &store).unwrap();
        assert_eq!(result.files_written, 1);

        // Verify the hard link was created.
        let original = tmp.path().join("original.txt");
        let copy = tmp.path().join("copy.txt");
        assert!(copy.exists());
        assert_eq!(fs::read(&copy).unwrap(), b"shared content");

        // Verify they share the same inode (hard link, not copy).
        use std::os::unix::fs::MetadataExt;
        let orig_meta = fs::metadata(&original).unwrap();
        let copy_meta = fs::metadata(&copy).unwrap();
        assert_eq!(orig_meta.ino(), copy_meta.ino());
    }

    #[cfg(unix)]
    #[test]
    fn execute_restore_hard_link_refuses_symlink_target() {
        // CVE-2026-32232 (ZeptoClaw) R3: the hard link target itself could
        // be a symlink to an external file. We must refuse to create a hard
        // link to a symlink target because it could alias an external inode.
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let store = repo.object_store();

        // Create a file outside the root.
        let outside = TempDir::new().unwrap();
        fs::write(outside.path().join("secret"), b"external data").unwrap();

        // Create a symlink inside the root pointing to the external file.
        std::os::unix::fs::symlink(
            outside.path().join("secret"),
            tmp.path().join("link_target"),
        )
        .unwrap();

        let plan = RestorePlan {
            actions: vec![RestoreAction::CreateHardLink {
                path: std::path::PathBuf::from("copy.txt"),
                target: std::path::PathBuf::from("link_target"),
            }],
            conflicts: vec![],
            warnings: vec![],
        };

        let err = execute_restore(&plan, tmp.path(), &store).unwrap_err();
        assert!(
            matches!(err, VarnError::InvalidPath(_)),
            "should refuse to hard link to a symlink target"
        );
    }
}
