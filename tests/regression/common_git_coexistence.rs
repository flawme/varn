//! Git coexistence regressions: the store must never leak into git, and
//! Varn must never touch git metadata.

use crate::common::TestRepo;
use std::fs;
use std::path::Path;

/// Create a minimal `.git` directory (no git binary needed).
fn make_git_dir(root: &Path) {
    fs::create_dir_all(root.join(".git")).unwrap();
}

#[test]
fn store_guard_created_on_init() {
    let repo = TestRepo::new();
    make_git_dir(repo.root());

    // Re-init inside a git repo: the guard must exist.
    let guard = repo.root().join(".varn/.gitignore");
    assert!(guard.is_file(), "store-level git guard must exist");
    assert_eq!(fs::read_to_string(&guard).unwrap(), "*\n");
}

#[test]
fn scanner_skips_varn_store() {
    let repo = TestRepo::new();
    repo.write("real.txt", b"real");
    // The store contains many files; none may appear in a scan.
    let scan = repo.scan();
    assert!(
        scan.entries
            .iter()
            .all(|e| !e.path.to_string_lossy().contains(".varn")),
        ".varn contents must never be scanned"
    );
}

#[test]
fn scanner_skips_git_internals() {
    // BUG 6: .git/HEAD, .git/objects, hooks etc. were being checkpointed.
    let repo = TestRepo::new();
    make_git_dir(repo.root());
    fs::write(repo.root().join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    fs::create_dir_all(repo.root().join(".git/objects/ab")).unwrap();
    fs::write(repo.root().join(".git/objects/ab/cdef"), b"object").unwrap();
    repo.write("real.txt", b"real");

    let scan = repo.scan();
    let paths: Vec<String> = scan
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        paths.iter().all(|p| !p.starts_with(".git/")),
        ".git internals must never be checkpointed: {paths:?}"
    );
    assert_eq!(paths, vec!["real.txt".to_string()]);
}

#[test]
fn varn_never_modifies_git_metadata() {
    let repo = TestRepo::new();
    make_git_dir(repo.root());
    let head = repo.root().join(".git/HEAD");
    fs::write(&head, b"ref: refs/heads/main\n").unwrap();
    let before = fs::read(&head).unwrap();

    repo.write("f.txt", b"data");
    let _ = repo.checkpoint("cp");
    let _ = repo.restore(&repo.checkpoint("cp2"));

    assert_eq!(
        fs::read(&head).unwrap(),
        before,
        ".git/HEAD must be untouched"
    );
}

#[test]
fn nested_varn_stores_are_skipped() {
    let repo = TestRepo::new();
    // A subdirectory with its own .varn (separately initialized project).
    fs::create_dir_all(repo.root().join("sub/.varn/objects")).unwrap();
    fs::write(repo.root().join("sub/.varn/config.json"), b"{}").unwrap();
    repo.write("sub/code.txt", b"code");

    let scan = repo.scan();
    let paths: Vec<String> = scan
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        paths.iter().all(|p| !p.contains(".varn")),
        "nested stores must be skipped: {paths:?}"
    );
    assert!(paths.contains(&"sub/code.txt".to_string()));
}

#[test]
fn unignored_store_warning_fires_and_clears() {
    let repo = TestRepo::new();
    make_git_dir(repo.root());

    // Simulate a legacy store without the guard.
    fs::remove_file(repo.root().join(".varn/.gitignore")).unwrap();

    let warning = varn::storage::coexistence_warning(&repo.repo.root, &repo.repo.varn_dir);
    assert!(warning.is_some(), "legacy store must warn");

    // Backfill via migrate's guard logic clears the warning.
    varn::storage::ensure_guard(&repo.repo.varn_dir).unwrap();
    assert!(varn::storage::coexistence_warning(&repo.repo.root, &repo.repo.varn_dir).is_none());
}
