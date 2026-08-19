//! Integration tests for the diff engine.
//!
//! These verify the pure-function diff logic over constructed entry lists,
//! covering added, modified, removed, and unchanged cases.

use std::path::PathBuf;
use varn::diff::{ChangeKind, diff_states};
use varn::filesystem::{EntryKind, EntryMeta, TreeEntry};

fn file_entry(path: &str, size: u64) -> TreeEntry {
    TreeEntry {
        path: PathBuf::from(path),
        meta: EntryMeta {
            kind: EntryKind::File,
            size,
            readonly: false,
            mtime: None,
            hash: None,
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
        },
    }
}

#[test]
fn diff_no_changes() {
    let old = vec![file_entry("a", 1), file_entry("b", 2)];
    let new = vec![file_entry("a", 1), file_entry("b", 2)];
    assert!(diff_states(&old, &new).is_empty());
}

#[test]
fn diff_detects_added_file() {
    let old = vec![file_entry("a", 1)];
    let new = vec![file_entry("a", 1), file_entry("b", 2)];
    let changes = diff_states(&old, &new);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Added);
    assert_eq!(changes[0].path, PathBuf::from("b"));
}

#[test]
fn diff_detects_removed_file() {
    let old = vec![file_entry("a", 1), file_entry("b", 2)];
    let new = vec![file_entry("a", 1)];
    let changes = diff_states(&old, &new);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Removed);
    assert_eq!(changes[0].path, PathBuf::from("b"));
}

#[test]
fn diff_detects_modified_file_size() {
    let old = vec![file_entry("a", 1)];
    let new = vec![file_entry("a", 2)];
    let changes = diff_states(&old, &new);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Modified);
    assert_eq!(changes[0].path, PathBuf::from("a"));
}

#[test]
fn diff_detects_modified_kind() {
    let old = vec![file_entry("a", 1)];
    let new = vec![dir_entry("a")];
    let changes = diff_states(&old, &new);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Modified);
}

#[test]
fn diff_detects_modified_readonly() {
    let old = vec![TreeEntry {
        path: PathBuf::from("a"),
        meta: EntryMeta {
            kind: EntryKind::File,
            size: 1,
            readonly: false,
            mtime: None,
            hash: None,
        },
    }];
    let new = vec![TreeEntry {
        path: PathBuf::from("a"),
        meta: EntryMeta {
            kind: EntryKind::File,
            size: 1,
            readonly: true,
            mtime: None,
            hash: None,
        },
    }];
    let changes = diff_states(&old, &new);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Modified);
}

#[test]
fn diff_handles_nested_paths() {
    let old = vec![file_entry("src/main.rs", 10)];
    let new = vec![file_entry("src/main.rs", 10), file_entry("src/new.rs", 5)];
    let changes = diff_states(&old, &new);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Added);
    assert_eq!(changes[0].path, PathBuf::from("src/new.rs"));
}

#[test]
fn diff_combined_scenario() {
    let old = vec![
        file_entry("keep.txt", 1),
        file_entry("modify.txt", 1),
        file_entry("delete.txt", 1),
    ];
    let new = vec![
        file_entry("keep.txt", 1),
        file_entry("modify.txt", 2),
        file_entry("add.txt", 1),
    ];
    let changes = diff_states(&old, &new);
    assert_eq!(changes.len(), 3);
    for change in &changes {
        match change.path.to_string_lossy().as_ref() {
            "modify.txt" => assert_eq!(change.kind, ChangeKind::Modified),
            "delete.txt" => assert_eq!(change.kind, ChangeKind::Removed),
            "add.txt" => assert_eq!(change.kind, ChangeKind::Added),
            other => panic!("unexpected path: {other}"),
        }
    }
}
