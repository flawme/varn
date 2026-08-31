//! Integration tests for git coexistence.
//!
//! These exercise the full end-to-end behavior: `varn init` inside a git
//! repository creates the store-level guard, the CLI warns for legacy stores
//! that lack it, `--gitignore` appends a root entry, and `varn migrate`
//! backfills the guard. Git itself is not invoked — the guard's effect on
//! Git is a documented property of gitignore semantics, and the unit tests
//! in `src/storage/git_guard.rs` cover the pattern matching.

use std::fs;
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;
use varn::cli::cmd_init;
use varn::storage::{Repo, coexistence_warning, ensure_guard, find_git_root, guard_present};

/// Create a minimal git repository at `root` without invoking git.
///
/// Git treats a directory containing `.git/` as a work tree; nothing else is
/// required for the guard logic, which only looks for the `.git` entry.
fn make_git_repo(root: &Path) {
    fs::create_dir_all(root.join(".git")).unwrap();
}

/// Run a checkpoint through the library path used by the CLI, returning the
/// snapshot ID.
fn run_checkpoint(root: &Path, description: &str) -> String {
    let repo = Repo::open(root).unwrap();
    let scanner = varn::filesystem::Scanner::with_ignore(&repo.root);
    let scan_result = scanner.scan().unwrap();
    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: description.to_string(),
        created_at: 1_000_000,
        root: repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, scan_result.entries);
    snapshot
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();
    snapshot.save(&repo.snapshots_dir()).unwrap();
    snapshot.meta.id.0
}

#[test]
fn init_inside_git_repo_creates_store_guard() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    make_git_repo(&root);

    cmd_init(&root, false, false).unwrap();

    let guard = root.join(".varn/.gitignore");
    assert!(guard.is_file(), "store-level guard must be created");
    assert_eq!(fs::read_to_string(&guard).unwrap(), "*\n");
    // The root .gitignore must be untouched by default.
    assert!(!root.join(".gitignore").exists());
}

#[test]
fn init_outside_git_repo_still_creates_guard() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("plain");
    fs::create_dir_all(&root).unwrap();

    cmd_init(&root, false, false).unwrap();

    assert!(root.join(".varn/.gitignore").is_file());
}

#[test]
fn init_outside_git_repo_with_gitignore_flag_fails() {
    // Skip when an enclosing repo exists above the temp dir (e.g. a git
    // repo at /tmp); the flag is then legitimately satisfiable.
    if find_git_root(TempDir::new().unwrap().path()).is_some() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("plain");
    fs::create_dir_all(&root).unwrap();

    let err = cmd_init(&root, true, false).unwrap_err();
    assert!(
        err.to_string().contains("no git repository"),
        "unexpected error: {err}"
    );
}

#[test]
fn init_with_gitignore_flag_appends_root_entry() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    make_git_repo(&root);
    fs::write(root.join(".gitignore"), b"target/\n").unwrap();

    cmd_init(&root, true, false).unwrap();

    let content = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert_eq!(content, "target/\n.varn/\n");
}

#[test]
fn legacy_store_without_guard_triggers_warning() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(root.join(".varn")).unwrap();
    make_git_repo(&root);

    let warning = coexistence_warning(&root, &root.join(".varn"));
    assert!(warning.is_some(), "legacy store must warn");
    let warning = warning.unwrap();
    assert!(warning.contains(".varn/"));
    assert!(warning.contains("gitignore"));
}

#[test]
fn migrate_backfills_guard_for_legacy_store() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(root.join(".varn")).unwrap();
    make_git_repo(&root);

    assert!(!guard_present(&root.join(".varn")));
    ensure_guard(&root.join(".varn")).unwrap();
    assert!(guard_present(&root.join(".varn")));

    // After the backfill the warning is gone.
    assert!(coexistence_warning(&root, &root.join(".varn")).is_none());
}

#[test]
fn checkpoint_warning_clears_after_guard_backfill() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    make_git_repo(&root);

    // Simulate a legacy store: config + dirs, no guard.
    let repo = Repo::init(&root, "linux").unwrap();
    fs::remove_file(repo.varn_dir.join(".gitignore")).unwrap();
    fs::write(root.join("file.txt"), b"hello").unwrap();

    assert!(coexistence_warning(&root, &repo.varn_dir).is_some());
    run_checkpoint(&root, "legacy");

    // Backfill via migrate's guard logic.
    ensure_guard(&repo.varn_dir).unwrap();
    assert!(coexistence_warning(&root, &repo.varn_dir).is_none());
}

#[test]
fn store_guard_makes_git_ignore_varn_dir() {
    // The real-world property: git status must not report .varn/ contents.
    // Skipped when git is unavailable.
    let Ok(_) = Command::new("git").arg("--version").output() else {
        return;
    };

    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();

    let ok = Command::new("git")
        .args(["init", "-q"])
        .current_dir(&root)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !ok {
        return;
    }

    cmd_init(&root, false, false).unwrap();
    fs::write(root.join("tracked.txt"), b"tracked").unwrap();

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&root)
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&output.stdout);

    assert!(
        status.contains("tracked.txt"),
        "tracked file must appear in git status; got: {status}"
    );
    assert!(
        !status.contains(".varn"),
        ".varn/ must NOT appear in git status; got: {status}"
    );
}

#[test]
fn find_git_root_matches_repo_layout() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    make_git_repo(&root);

    // find_git_root returns a fully resolved path; canonicalize the
    // expectation so the comparison holds on macOS (/var -> /private/var).
    let expected = fs::canonicalize(&root).unwrap_or_else(|_| root.clone());
    assert_eq!(find_git_root(&root), Some(expected));
}
