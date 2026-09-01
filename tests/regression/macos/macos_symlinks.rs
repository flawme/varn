//! macOS symlink semantics: /var -> /private/var resolution and symlinked
//! directories.

use crate::common::TestRepo;
use std::fs;
use std::path::PathBuf;

#[test]
fn symlink_round_trip_with_canonicalized_root() {
    // find_git_root canonicalizes paths; on macOS /var is a symlink to
    // /private/var. Ensure restore works when the repo root path contains
    // symlinked components (TempDir on macOS IS under /var/...).
    let repo = TestRepo::new();
    repo.write("target.txt", b"content");
    std::os::unix::fs::symlink(PathBuf::from("target.txt"), repo.root().join("link.txt")).unwrap();

    let snapshot = repo.checkpoint("macos link");
    fs::remove_file(repo.root().join("link.txt")).unwrap();
    repo.restore(&snapshot);

    assert!(repo.root().join("link.txt").is_symlink());
    assert_eq!(
        fs::read_link(repo.root().join("link.txt")).unwrap(),
        PathBuf::from("target.txt")
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn git_root_detection_with_symlinked_temp() {
    // The macOS CI failure class: tests compared unresolved TempDir paths
    // against find_git_root's canonicalized result.
    let repo = TestRepo::new();
    fs::create_dir_all(repo.root().join(".git")).unwrap();

    let found = varn::storage::find_git_root(&repo.repo.root);
    assert!(found.is_some(), "git root must be found");
    // The found root must EXIST as a directory (canonicalized).
    assert!(found.unwrap().is_dir());
}

#[test]
fn resource_fork_free_round_trip() {
    // Plain files (no resource forks) round-trip exactly; pinned so a
    // future xattr feature cannot silently change content handling.
    let repo = TestRepo::new();
    let content: Vec<u8> = (0..=255u8).collect();
    repo.write("data.bin", &content);

    let snapshot = repo.checkpoint("binary");
    fs::remove_file(repo.root().join("data.bin")).unwrap();
    repo.restore(&snapshot);
    assert_eq!(repo.read("data.bin"), content);
    assert!(repo.verifies(&snapshot));
}
