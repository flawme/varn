//! Regression tests for directory metadata restoration.
//!
//! Creating, writing, or deleting entries inside a directory updates the
//! directory's own mtime. Restore must therefore apply directory metadata
//! in a post-order pass (after all children), or the restored mtime is
//! clobbered and verification fails. This bit on Windows CI where the
//! checkpoint→modify→restore sequence crossed a second boundary; the
//! deterministic test below forces the drift explicitly on every platform.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use varn::filesystem::Scanner;
use varn::storage::Repo;

/// Set a directory's mtime to a fixed old timestamp.
fn set_dir_mtime(path: &Path, unix_secs: i64) {
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(unix_secs, 0)).unwrap();
}

/// Read a directory's mtime as unix seconds (the same truncation the
/// scanner uses).
fn dir_mtime(path: &Path) -> i64 {
    fs::symlink_metadata(path)
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[test]
fn restore_restores_directory_mtime_after_child_operations() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // Old, fixed mtime: far in the past so it can never collide with "now".
    const OLD_MTIME: i64 = 1_000_000_000; // 2001-09-09

    fs::create_dir_all(tmp.path().join("src")).unwrap();
    fs::write(tmp.path().join("src/main.rs"), b"fn main() {}").unwrap();
    set_dir_mtime(&tmp.path().join("src"), OLD_MTIME);
    assert_eq!(dir_mtime(&tmp.path().join("src")), OLD_MTIME);

    // Checkpoint captures the directory with the old mtime.
    let scanner = Scanner::new(&repo.root);
    let scan = scanner.scan().unwrap();
    let dir_entry = scan
        .entries
        .iter()
        .find(|e| e.path == Path::new("src"))
        .expect("src must be scanned");
    assert_eq!(dir_entry.meta.mtime, Some(OLD_MTIME));

    let snapshot = varn::snapshot::SnapshotData::new(
        varn::core::CheckpointMeta {
            id: varn::core::CheckpointId("pending".to_string()),
            description: "dir mtime".to_string(),
            created_at: 1,
            root: repo.root.clone(),
        },
        scan.entries.clone(),
    );
    snapshot
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();

    // Simulate an agent: add and remove a child. This bumps src/'s mtime to
    // "now" — deterministically different from OLD_MTIME.
    fs::write(tmp.path().join("src/new.rs"), b"pub fn new() {}").unwrap();
    fs::remove_file(tmp.path().join("src/main.rs")).unwrap();
    let drifted = dir_mtime(&tmp.path().join("src"));
    assert_ne!(
        drifted, OLD_MTIME,
        "child operations must bump the directory mtime for this test to be meaningful"
    );

    // Plan and execute the restore.
    let current = Scanner::new(&repo.root).scan().unwrap();
    let plan = varn::restore::plan_restore(&snapshot.entries, &current.entries);
    varn::restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // The directory mtime must be back to the checkpointed value — this is
    // the assertion that failed on Windows before the ApplyDirMeta pass.
    assert_eq!(
        dir_mtime(&tmp.path().join("src")),
        OLD_MTIME,
        "directory mtime must be restored after child operations"
    );

    // And full verification passes.
    assert!(
        varn::restore::verify_restore(&repo.root, &snapshot.entries),
        "verification must pass after directory metadata pass"
    );
}

#[test]
fn restore_nested_dir_meta_applies_deepest_first() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    const OLD_MTIME: i64 = 1_100_000_000;

    fs::create_dir_all(tmp.path().join("a/b")).unwrap();
    fs::write(tmp.path().join("a/b/f.txt"), b"x").unwrap();
    set_dir_mtime(&tmp.path().join("a/b"), OLD_MTIME);
    set_dir_mtime(&tmp.path().join("a"), OLD_MTIME);

    let scanner = Scanner::new(&repo.root);
    let scan = scanner.scan().unwrap();
    let snapshot = varn::snapshot::SnapshotData::new(
        varn::core::CheckpointMeta {
            id: varn::core::CheckpointId("pending".to_string()),
            description: "nested".to_string(),
            created_at: 1,
            root: repo.root.clone(),
        },
        scan.entries.clone(),
    );
    snapshot
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();

    // Drift both directories.
    fs::write(tmp.path().join("a/b/g.txt"), b"y").unwrap();
    fs::remove_file(tmp.path().join("a/b/f.txt")).unwrap();

    let current = Scanner::new(&repo.root).scan().unwrap();
    let plan = varn::restore::plan_restore(&snapshot.entries, &current.entries);
    varn::restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    assert_eq!(dir_mtime(&tmp.path().join("a/b")), OLD_MTIME);
    assert_eq!(dir_mtime(&tmp.path().join("a")), OLD_MTIME);
    assert!(varn::restore::verify_restore(&repo.root, &snapshot.entries));
}
