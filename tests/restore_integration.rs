//! Integration tests for the restore engine.
//!
//! These exercise the full restore workflow: checkpoint → modify → plan →
//! execute → verify. They cover file restoration from the object store,
//! deletion of unexpected files, conflict detection, and post-restore
//! verification.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use varn::filesystem::Scanner;
use varn::restore::{self, Conflict, RestoreAction, RestorePlan};
use varn::snapshot::SnapshotData;
use varn::storage::Repo;

/// Helper: create a file with content.
fn write_file(root: &Path, path: &str, content: &[u8]) {
    let full = root.join(path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(&full, content).unwrap();
}

/// Helper: create a checkpoint and store its content blobs.
fn checkpoint(repo: &Repo, description: &str) -> SnapshotData {
    let scanner = Scanner::new(&repo.root);
    let scan = scanner.scan().unwrap();

    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: description.to_string(),
        created_at: 1_000_000,
        root: repo.root.clone(),
    };
    let snapshot = SnapshotData::new(meta, scan.entries);
    snapshot
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    snapshot.save(&repo.snapshots_dir()).unwrap();
    snapshot
}

/// Helper: scan the current state and plan a restore against a snapshot.
fn plan_against_current(repo: &Repo, snapshot: &SnapshotData) -> RestorePlan {
    let scanner = Scanner::new(&repo.root);
    let current = scanner.scan().unwrap();
    restore::plan_restore(&snapshot.entries, &current.entries)
}

#[test]
fn restore_modified_file() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"original");

    let snapshot = checkpoint(&repo, "before change");

    // Modify the file.
    write_file(tmp.path(), "a.txt", b"modified");

    let plan = plan_against_current(&repo, &snapshot);
    assert!(plan.has_conflicts());
    assert!(
        plan.conflicts
            .iter()
            .any(|c| matches!(c, Conflict::Modified { .. }))
    );

    // Execute the restore.
    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert!(result.files_written > 0);

    // File should be restored to original content.
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "original"
    );

    // Verify.
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_deleted_file() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"keep me");
    write_file(tmp.path(), "b.txt", b"also keep");

    let snapshot = checkpoint(&repo, "before deletion");

    // Delete a file.
    fs::remove_file(tmp.path().join("b.txt")).unwrap();

    let plan = plan_against_current(&repo, &snapshot);
    assert!(!plan.has_conflicts());
    assert!(
        plan.actions
            .iter()
            .any(|a| matches!(a, RestoreAction::WriteFile { .. }))
    );

    // Execute the restore.
    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert!(result.files_written > 0);

    // File should be restored.
    assert_eq!(
        fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "also keep"
    );
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_deletes_unexpected_files() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"original");

    let snapshot = checkpoint(&repo, "before");

    // Add an unexpected file.
    write_file(tmp.path(), "unexpected.txt", b"should be deleted");

    let plan = plan_against_current(&repo, &snapshot);
    assert!(plan.has_conflicts());
    assert!(
        plan.conflicts
            .iter()
            .any(|c| matches!(c, Conflict::Unexpected { .. }))
    );

    // Execute the restore.
    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert!(result.deleted > 0);

    // Unexpected file should be gone.
    assert!(!tmp.path().join("unexpected.txt").exists());
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_no_changes_is_noop() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"stable");

    let snapshot = checkpoint(&repo, "baseline");

    // No changes.
    let plan = plan_against_current(&repo, &snapshot);
    assert!(plan.actions.is_empty());
    assert!(!plan.has_conflicts());

    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert_eq!(result.files_written, 0);
    assert_eq!(result.deleted, 0);
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_combined_scenario() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "keep.txt", b"unchanged");
    write_file(tmp.path(), "modify.txt", b"original");
    write_file(tmp.path(), "delete.txt", b"to be deleted");

    let snapshot = checkpoint(&repo, "before changes");

    // Make changes.
    write_file(tmp.path(), "modify.txt", b"changed");
    fs::remove_file(tmp.path().join("delete.txt")).unwrap();
    write_file(tmp.path(), "add.txt", b"new file");

    let plan = plan_against_current(&repo, &snapshot);

    // Should have conflicts: modify.txt (modified), add.txt (unexpected).
    assert!(plan.has_conflicts());

    // Execute the restore.
    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // Verify the restore.
    assert!(
        restore::verify_restore(&repo.root, &snapshot.entries),
        "filesystem should match snapshot after restore"
    );

    // Check specific files.
    assert_eq!(
        fs::read_to_string(tmp.path().join("keep.txt")).unwrap(),
        "unchanged"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("modify.txt")).unwrap(),
        "original"
    );
    assert!(tmp.path().join("delete.txt").exists());
    assert!(!tmp.path().join("add.txt").exists());
    assert!(result.files_written > 0);
    assert!(result.deleted > 0);
}

#[test]
fn restore_recreates_nested_directories() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    fs::create_dir_all(tmp.path().join("a/b/c")).unwrap();
    write_file(tmp.path(), "a/b/c/deep.txt", b"deep content");

    let snapshot = checkpoint(&repo, "nested");

    // Delete the entire tree.
    fs::remove_dir_all(tmp.path().join("a")).unwrap();

    let plan = plan_against_current(&repo, &snapshot);

    // Execute the restore.
    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert!(result.dirs_created > 0);
    assert!(result.files_written > 0);

    // File should be restored.
    assert_eq!(
        fs::read_to_string(tmp.path().join("a/b/c/deep.txt")).unwrap(),
        "deep content"
    );
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_preserves_unchanged_files() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"content a");
    write_file(tmp.path(), "b.txt", b"content b");

    let snapshot = checkpoint(&repo, "baseline");

    // Only modify a.txt.
    write_file(tmp.path(), "a.txt", b"changed");

    let plan = plan_against_current(&repo, &snapshot);
    restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // b.txt should be untouched.
    assert_eq!(
        fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "content b"
    );
}

#[test]
fn restore_verification_detects_failure() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"original");

    let snapshot = checkpoint(&repo, "baseline");

    // Modify after checkpoint but don't restore.
    write_file(tmp.path(), "a.txt", b"modified");

    // Verification should fail because current state != snapshot.
    assert!(!restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_empty_workspace() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    let snapshot = checkpoint(&repo, "empty");

    let plan = plan_against_current(&repo, &snapshot);
    assert!(plan.actions.is_empty());
    assert!(!plan.has_conflicts());

    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert_eq!(result.files_written, 0);
    assert_eq!(result.deleted, 0);
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_deletes_unexpected_directory() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"keep");

    let snapshot = checkpoint(&repo, "before");

    // Add an unexpected directory with files.
    write_file(tmp.path(), "unexpected/inner.txt", b"delete me");

    let plan = plan_against_current(&repo, &snapshot);
    assert!(plan.has_conflicts());

    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert!(result.deleted > 0);
    assert!(!tmp.path().join("unexpected").exists());
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_multiple_files() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"content a");
    write_file(tmp.path(), "b.txt", b"content b");
    write_file(tmp.path(), "c.txt", b"content c");

    let snapshot = checkpoint(&repo, "three files");

    // Delete all files.
    fs::remove_file(tmp.path().join("a.txt")).unwrap();
    fs::remove_file(tmp.path().join("b.txt")).unwrap();
    fs::remove_file(tmp.path().join("c.txt")).unwrap();

    let plan = plan_against_current(&repo, &snapshot);
    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert_eq!(result.files_written, 3);

    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "content a"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "content b"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("c.txt")).unwrap(),
        "content c"
    );
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_deduplication_uses_same_object() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "a.txt", b"identical");
    write_file(tmp.path(), "b.txt", b"identical");

    let snapshot = checkpoint(&repo, "dedup");

    // Delete both files.
    fs::remove_file(tmp.path().join("a.txt")).unwrap();
    fs::remove_file(tmp.path().join("b.txt")).unwrap();

    let plan = plan_against_current(&repo, &snapshot);
    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert_eq!(result.files_written, 2);

    // Both files should have the same content.
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "identical"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "identical"
    );
    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[test]
fn restore_full_workflow_checkpoint_modify_restore() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Set up initial state.
    write_file(tmp.path(), "src/main.rs", b"fn main() {}");
    write_file(tmp.path(), "src/lib.rs", b"pub fn lib() {}");
    write_file(tmp.path(), "Cargo.toml", b"[package]");
    fs::create_dir_all(tmp.path().join("tests")).unwrap();
    write_file(
        tmp.path(),
        "tests/integration.rs",
        b"#[test] fn it_works() {}",
    );

    let snapshot = checkpoint(&repo, "initial state");

    // Simulate an agent making changes.
    write_file(
        tmp.path(),
        "src/main.rs",
        b"fn main() { println!(\"hello\"); }",
    );
    fs::remove_file(tmp.path().join("src/lib.rs")).unwrap();
    write_file(tmp.path(), "src/new_file.rs", b"pub fn new() {}");
    write_file(tmp.path(), "Cargo.toml", b"[package]\nname = \"changed\"");

    // Plan and execute the restore.
    let plan = plan_against_current(&repo, &snapshot);
    let _result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // Verify the restore.
    assert!(
        restore::verify_restore(&repo.root, &snapshot.entries),
        "filesystem should match snapshot after restore"
    );

    // Check specific files are restored.
    assert_eq!(
        fs::read_to_string(tmp.path().join("src/main.rs")).unwrap(),
        "fn main() {}"
    );
    assert!(tmp.path().join("src/lib.rs").exists());
    assert!(!tmp.path().join("src/new_file.rs").exists());
    assert_eq!(
        fs::read_to_string(tmp.path().join("Cargo.toml")).unwrap(),
        "[package]"
    );
}

#[cfg(unix)]
#[test]
fn restore_symlink_from_checkpoint() {
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "target.txt", b"target content");
    symlink(tmp.path().join("target.txt"), tmp.path().join("link.txt")).unwrap();

    let snapshot = checkpoint(&repo, "with symlink");

    // Delete the symlink.
    fs::remove_file(tmp.path().join("link.txt")).unwrap();

    let plan = plan_against_current(&repo, &snapshot);
    assert!(
        plan.actions
            .iter()
            .any(|a| matches!(a, RestoreAction::CreateSymlink { .. })),
        "plan should include a CreateSymlink action"
    );

    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert_eq!(result.symlinks_created, 1);

    // The symlink should be restored.
    let link_path = tmp.path().join("link.txt");
    assert!(link_path.is_symlink());
    assert_eq!(
        fs::read_link(&link_path).unwrap(),
        tmp.path().join("target.txt")
    );

    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[cfg(unix)]
#[test]
fn restore_symlink_with_changed_target() {
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    write_file(tmp.path(), "original_target.txt", b"original");
    symlink(
        tmp.path().join("original_target.txt"),
        tmp.path().join("link.txt"),
    )
    .unwrap();

    let snapshot = checkpoint(&repo, "symlink v1");

    // Change the symlink target.
    fs::remove_file(tmp.path().join("link.txt")).unwrap();
    write_file(tmp.path(), "new_target.txt", b"new");
    symlink(
        tmp.path().join("new_target.txt"),
        tmp.path().join("link.txt"),
    )
    .unwrap();

    let plan = plan_against_current(&repo, &snapshot);
    assert!(plan.has_conflicts(), "changed symlink should be a conflict");

    let result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();
    assert_eq!(result.symlinks_created, 1);

    // The symlink should point to the original target.
    let link_path = tmp.path().join("link.txt");
    assert!(link_path.is_symlink());
    assert_eq!(
        fs::read_link(&link_path).unwrap(),
        tmp.path().join("original_target.txt")
    );

    assert!(restore::verify_restore(&repo.root, &snapshot.entries));
}

#[cfg(unix)]
#[test]
fn restore_full_workflow_with_symlinks() {
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Set up state with files and symlinks.
    write_file(tmp.path(), "src/main.rs", b"fn main() {}");
    write_file(tmp.path(), "src/config.txt", b"config");
    symlink(
        tmp.path().join("src/config.txt"),
        tmp.path().join("config_link"),
    )
    .unwrap();
    symlink(
        Path::new("../src/main.rs"),
        tmp.path().join("src/main_link"),
    )
    .unwrap();

    let snapshot = checkpoint(&repo, "initial state");

    // Make changes: modify file, delete symlink, add new file.
    write_file(
        tmp.path(),
        "src/main.rs",
        b"fn main() { println!(\"hi\"); }",
    );
    fs::remove_file(tmp.path().join("config_link")).unwrap();
    write_file(tmp.path(), "new_file.txt", b"new");

    // Plan and execute the restore.
    let plan = plan_against_current(&repo, &snapshot);
    let _result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // Verify the restore.
    assert!(
        restore::verify_restore(&repo.root, &snapshot.entries),
        "filesystem should match snapshot after restore"
    );

    // Check the symlink is restored.
    let link_path = tmp.path().join("config_link");
    assert!(link_path.is_symlink());
    assert_eq!(
        fs::read_link(&link_path).unwrap(),
        tmp.path().join("src/config.txt")
    );

    // Check the relative symlink is restored.
    let main_link = tmp.path().join("src/main_link");
    assert!(main_link.is_symlink());
    assert_eq!(
        fs::read_link(&main_link).unwrap(),
        Path::new("../src/main.rs")
    );
}

// ---------------------------------------------------------------------------
// Adversarial tests: vulnerabilities found during security review
// ---------------------------------------------------------------------------

/// Symlink escape prevention (CVE-2026-71556 / GHSA-9qw7-j9xw-fv9c pattern).
///
/// A crafted restore plan contains a symlink whose target points outside
/// the managed root, followed by a file write through that symlink. The
/// restore engine must refuse to write through a symlink in the leading
/// path.
#[cfg(unix)]
#[test]
fn restore_blocks_symlink_escape_in_leading_path() {
    let tmp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    let store = repo.object_store();

    // Store the payload content.
    let payload = b"escaped";
    let hash = varn::filesystem::hash_bytes(payload);
    store.store_content(&hash, payload).unwrap();

    // The plan tries to:
    // 1. Create a symlink "evil" -> outside_dir
    // 2. Write a file "evil/payload.txt" (which would follow the symlink)
    let plan = RestorePlan {
        actions: vec![
            RestoreAction::CreateSymlink {
                path: Path::new("evil").to_path_buf(),
                target: outside.path().to_path_buf(),
            },
            RestoreAction::WriteFile {
                path: Path::new("evil/payload.txt").to_path_buf(),
                hash: hash.clone(),
                readonly: false,
                mtime: None,
            },
        ],
        conflicts: vec![],
        warnings: vec![],
    };

    let result = restore::execute_restore(&plan, &repo.root, &store);

    // The restore must fail — it should refuse to write through the symlink.
    assert!(
        result.is_err(),
        "restore must refuse to write through a symlink in the leading path"
    );

    // No file should have escaped to the outside directory.
    assert!(
        !outside.path().join("payload.txt").exists(),
        "no file should escape the managed root"
    );
}

/// Symlink escape: even if the symlink already exists on disk (not created
/// by the restore plan), writing through it must still be blocked.
#[cfg(unix)]
#[test]
fn restore_blocks_write_through_existing_symlink() {
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    let store = repo.object_store();

    // Pre-create a symlink pointing outside the root.
    symlink(outside.path(), tmp.path().join("evil")).unwrap();

    let payload = b"escaped";
    let hash = varn::filesystem::hash_bytes(payload);
    store.store_content(&hash, payload).unwrap();

    let plan = RestorePlan {
        actions: vec![RestoreAction::WriteFile {
            path: Path::new("evil/payload.txt").to_path_buf(),
            hash,
            readonly: false,
            mtime: None,
        }],
        conflicts: vec![],
        warnings: vec![],
    };

    let result = restore::execute_restore(&plan, &repo.root, &store);
    assert!(
        result.is_err(),
        "must refuse to write through existing symlink"
    );
    assert!(
        !outside.path().join("payload.txt").exists(),
        "no file should escape"
    );
}

/// Pre-flight check: if an object is missing from the store, restore must
/// fail BEFORE modifying any files (atomic failure).
#[test]
fn restore_fails_atomically_when_object_missing() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    let store = repo.object_store();

    // Store content for file A but NOT for file B.
    let hash_a = varn::filesystem::hash_bytes(b"content_a");
    store.store_content(&hash_a, b"content_a").unwrap();
    let hash_b = varn::filesystem::hash_bytes(b"content_b");
    // Deliberately do NOT store hash_b.

    // Both files currently have "modified" content.
    write_file(tmp.path(), "a.txt", b"modified_a");
    write_file(tmp.path(), "b.txt", b"modified_b");

    let plan = RestorePlan {
        actions: vec![
            RestoreAction::WriteFile {
                path: Path::new("a.txt").to_path_buf(),
                hash: hash_a,
                readonly: false,
                mtime: None,
            },
            RestoreAction::WriteFile {
                path: Path::new("b.txt").to_path_buf(),
                hash: hash_b,
                readonly: false,
                mtime: None,
            },
        ],
        conflicts: vec![],
        warnings: vec![],
    };

    let result = restore::execute_restore(&plan, &repo.root, &store);

    // Must fail.
    assert!(
        result.is_err(),
        "restore must fail when an object is missing"
    );

    // No files should have been modified — the pre-flight check prevents
    // partial restores.
    assert_eq!(
        fs::read_to_string(tmp.path().join("a.txt")).unwrap(),
        "modified_a",
        "a.txt must not be modified if the restore cannot complete"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("b.txt")).unwrap(),
        "modified_b",
        "b.txt must not be modified"
    );
}

/// Permissions (readonly flag) must be restored.
#[cfg(unix)]
#[test]
fn restore_restores_readonly_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Create a read-only file.
    write_file(tmp.path(), "readonly.txt", b"important");
    let mut perms = fs::metadata(tmp.path().join("readonly.txt"))
        .unwrap()
        .permissions();
    perms.set_mode(0o444);
    fs::set_permissions(tmp.path().join("readonly.txt"), perms).unwrap();

    let snapshot = checkpoint(&repo, "with readonly");

    // Make the file writable.
    let mut perms = fs::metadata(tmp.path().join("readonly.txt"))
        .unwrap()
        .permissions();
    perms.set_mode(0o644);
    fs::set_permissions(tmp.path().join("readonly.txt"), perms).unwrap();

    // Restore.
    let plan = plan_against_current(&repo, &snapshot);
    let _result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // The file should be read-only again.
    let mode = fs::metadata(tmp.path().join("readonly.txt"))
        .unwrap()
        .permissions()
        .mode();
    assert!(
        mode & 0o200 == 0,
        "file should be read-only after restore (mode={:o})",
        mode
    );
}

/// mtime must be restored.
#[test]
fn restore_restores_mtime() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    write_file(tmp.path(), "timed.txt", b"timestamped");

    // Set a specific mtime using filetime.
    let old_time = filetime::FileTime::from_unix_time(1_600_000_000, 0);
    filetime::set_file_mtime(tmp.path().join("timed.txt"), old_time).unwrap();

    let snapshot = checkpoint(&repo, "with mtime");

    // Touch the file to change its mtime.
    filetime::set_file_mtime(
        tmp.path().join("timed.txt"),
        filetime::FileTime::from_unix_time(1_700_000_000, 0),
    )
    .unwrap();

    // Restore.
    let plan = plan_against_current(&repo, &snapshot);
    let _result = restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // The mtime should be restored.
    let meta = fs::metadata(tmp.path().join("timed.txt")).unwrap();
    let mtime = meta
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert_eq!(
        mtime, 1_600_000_000,
        "mtime should be restored to the checkpoint value"
    );
}
