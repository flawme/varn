//! Nested-directory regressions: creation order, deletion of trees,
//! metadata of intermediate directories.

use crate::common::{TestRepo, get_mtime, set_mtime};
use std::fs;

#[test]
fn nested_tree_round_trip() {
    let repo = TestRepo::new();
    repo.write("l1/l2/l3/l4/file.txt", b"deep");
    repo.write("l1/other.txt", b"other");
    repo.write("top.txt", b"top");

    let snapshot = repo.checkpoint("nested");

    fs::remove_dir_all(repo.root().join("l1")).unwrap();
    fs::remove_file(repo.root().join("top.txt")).unwrap();

    repo.restore(&snapshot);
    assert_eq!(repo.read_str("l1/l2/l3/l4/file.txt"), "deep");
    assert_eq!(repo.read_str("l1/other.txt"), "other");
    assert_eq!(repo.read_str("top.txt"), "top");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn nested_tree_with_metadata_round_trip() {
    let repo = TestRepo::new();
    const OLD: i64 = 1_200_000_000;

    repo.write("x/y/z/f.txt", b"deep");
    set_mtime(&repo.root().join("x/y/z"), OLD);
    set_mtime(&repo.root().join("x/y"), OLD);
    set_mtime(&repo.root().join("x"), OLD);

    let snapshot = repo.checkpoint("nested meta");

    repo.write("x/y/z/g.txt", b"new");
    fs::remove_file(repo.root().join("x/y/z/f.txt")).unwrap();

    repo.restore(&snapshot);

    assert_eq!(get_mtime(&repo.root().join("x/y/z")), Some(OLD));
    assert_eq!(get_mtime(&repo.root().join("x/y")), Some(OLD));
    assert_eq!(get_mtime(&repo.root().join("x")), Some(OLD));
    assert!(repo.verifies(&snapshot));
}

#[test]
fn directory_replaced_by_file_is_handled() {
    let repo = TestRepo::new();
    repo.write("thing/inner.txt", b"dir content");
    let snapshot = repo.checkpoint("dir");

    // Replace the directory with a file of the same name: kind change.
    fs::remove_dir_all(repo.root().join("thing")).unwrap();
    fs::write(repo.root().join("thing"), b"now a file").unwrap();

    // Restore must detect the conflict and (with confirmation skipped in
    // the API) replace the file with the directory.
    repo.restore(&snapshot);
    assert!(repo.root().join("thing").is_dir());
    assert_eq!(repo.read_str("thing/inner.txt"), "dir content");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn file_replaced_by_directory_is_handled() {
    let repo = TestRepo::new();
    repo.write("thing", b"file content");
    let snapshot = repo.checkpoint("file");

    fs::remove_file(repo.root().join("thing")).unwrap();
    fs::create_dir_all(repo.root().join("thing")).unwrap();
    repo.write("thing/inner.txt", b"now a dir");

    let plan = repo.plan_restore(&snapshot);
    match varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store()) {
        Ok(_) => {}
        Err(e) => panic!("restore failed: {e}"),
    }
    assert!(repo.root().join("thing").is_file());
    assert_eq!(repo.read_str("thing"), "file content");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn empty_directories_are_captured_and_restored() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.root().join("empty/nested")).unwrap();
    repo.write("real.txt", b"real");

    let snapshot = repo.checkpoint("empty dirs");
    let empty_dir_entry = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("empty/nested"));
    assert!(empty_dir_entry.is_some(), "empty dirs must be captured");
    assert_eq!(
        empty_dir_entry.unwrap().meta.kind,
        varn::filesystem::EntryKind::Directory
    );

    fs::remove_dir_all(repo.root().join("empty")).unwrap();
    repo.restore(&snapshot);
    assert!(repo.root().join("empty/nested").is_dir());
    assert!(repo.verifies(&snapshot));
}

#[test]
fn deep_delete_then_restore_preserves_siblings() {
    let repo = TestRepo::new();
    repo.write("keep/a.txt", b"a");
    repo.write("drop/b.txt", b"b");
    let snapshot = repo.checkpoint("mixed");

    fs::remove_dir_all(repo.root().join("drop")).unwrap();
    repo.restore(&snapshot);

    assert_eq!(repo.read_str("keep/a.txt"), "a");
    assert_eq!(repo.read_str("drop/b.txt"), "b");
    assert!(repo.verifies(&snapshot));
}
