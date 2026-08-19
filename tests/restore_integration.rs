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
