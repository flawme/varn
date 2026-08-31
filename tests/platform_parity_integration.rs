//! Integration tests for cross-platform metadata parity.
//!
//! Unix-only tests here cover full mode restoration. macOS BSD flags and
//! Windows attributes/ACLs are exercised by cfg-gated tests in the platform
//! module and by CI on each OS.

// Everything in this file except the serde round-trip tests is Unix-only;
// gate the whole file so Windows builds have no unused imports.
#![cfg(unix)]

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use varn::filesystem::Scanner;
use varn::storage::Repo;

#[cfg(unix)]
#[test]
fn restore_applies_full_unix_mode() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    // A file with an executable mode.
    let file = tmp.path().join("script.sh");
    fs::write(&file, b"#!/bin/sh\necho hi\n").unwrap();
    let mut perms = fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&file, perms).unwrap();

    // Checkpoint.
    let scanner = Scanner::new(&repo.root);
    let scan = scanner.scan().unwrap();
    let entry = scan
        .entries
        .iter()
        .find(|e| e.path == Path::new("script.sh"))
        .expect("script.sh must be scanned");
    assert_eq!(entry.meta.mode, Some(0o755), "mode must be captured");

    // Store content so restore can read it back.
    let snapshot = varn::snapshot::SnapshotData::new(
        varn::core::CheckpointMeta {
            id: varn::core::CheckpointId("t".to_string()),
            description: "mode test".to_string(),
            created_at: 1,
            root: repo.root.clone(),
        },
        scan.entries.clone(),
    );
    snapshot
        .store_content_blobs(&repo.root, &repo.object_store())
        .unwrap();

    // Change the mode on disk (simulating drift), then re-scan so the
    // "current" state reflects the drift.
    let mut perms = fs::metadata(&file).unwrap().permissions();
    perms.set_mode(0o600);
    fs::set_permissions(&file, perms).unwrap();
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let current = Scanner::new(&repo.root).scan().unwrap();

    // Plan a restore from the snapshot state.
    let plan = varn::restore::plan_restore(&snapshot.entries, &current.entries);
    assert!(
        plan.actions.iter().any(|a| matches!(
            a,
            varn::restore::RestoreAction::WriteFile {
                mode: Some(0o755),
                ..
            }
        )),
        "plan must carry the snapshot mode; actions: {:?}",
        plan.actions
    );
    let result = varn::restore::execute_restore(&plan, &repo.root, &repo.object_store()).unwrap();

    // The full mode must be restored, not just the readonly bit.
    let restored_mode = fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(restored_mode, 0o755, "full mode must be restored");
    assert!(
        !result.warnings.iter().any(|w| w.contains("mode")),
        "no mode warnings expected: {:?}",
        result.warnings
    );
}

#[cfg(unix)]
#[test]
fn restore_mode_falls_back_to_readonly_for_legacy_snapshots() {
    use std::os::unix::fs::PermissionsExt;

    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    let store = repo.object_store();

    // A legacy snapshot entry: no mode captured (pre-0.1.2), readonly=true.
    let hash = varn::filesystem::hash_bytes(b"legacy");
    store.store_content(&hash, b"legacy").unwrap();

    let entry = TreeEntryBuilder::new("legacy.txt")
        .readonly(true)
        .hash(&hash)
        .build();

    let plan = varn::restore::plan_restore(&[entry], &[]);
    varn::restore::execute_restore(&plan, &repo.root, &store).unwrap();

    let mode = fs::metadata(repo.root.join("legacy.txt"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(
        mode & 0o200,
        0,
        "readonly fallback must clear the owner write bit"
    );
}

#[cfg(unix)]
#[test]
fn scan_captures_mode_for_directories_and_files() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("sub")).unwrap();
    fs::write(tmp.path().join("sub/f.txt"), b"x").unwrap();

    let scan = Scanner::new(tmp.path()).scan().unwrap();
    for entry in &scan.entries {
        assert!(
            entry.meta.mode.is_some(),
            "mode must be captured on Unix for {}",
            entry.path.display()
        );
    }
}

/// Test that snapshots written by older Varn versions (without mode/flags/
/// attributes/acl fields) still deserialize. This is the backward
/// compatibility contract for the storage format.
#[test]
fn snapshot_json_without_new_fields_deserializes() {
    let legacy = r#"{
        "kind": "file",
        "size": 5,
        "readonly": false,
        "mtime": 1700000000,
        "hash": "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        "nlink": 1
    }"#;
    let meta: varn::filesystem::EntryMeta = serde_json::from_str(legacy).unwrap();
    assert_eq!(meta.mode, None);
    assert_eq!(meta.flags, None);
    assert_eq!(meta.attributes, None);
    assert_eq!(meta.acl, None);
    assert_eq!(meta.nlink, 1);
}

/// New fields round-trip through JSON.
#[test]
fn entry_meta_with_new_fields_round_trips() {
    let meta = varn::filesystem::EntryMeta {
        kind: varn::filesystem::EntryKind::File,
        size: 5,
        readonly: false,
        mtime: Some(1700000000),
        hash: Some("abc".to_string()),
        target: None,
        nlink: 1,
        hardlink_to: None,
        uid: Some(1000),
        gid: Some(1000),
        mode: Some(0o755),
        flags: Some(0x2),
        attributes: Some(0x20),
        acl: Some("O:S-1-5-21".to_string()),
    };
    let json = serde_json::to_string(&meta).unwrap();
    let back: varn::filesystem::EntryMeta = serde_json::from_str(&json).unwrap();
    assert_eq!(back, meta);
}

/// Helper for building test entries without repeating every field.
#[cfg(unix)]
struct TreeEntryBuilder {
    path: &'static str,
    readonly: bool,
    hash: Option<String>,
}

#[cfg(unix)]
impl TreeEntryBuilder {
    fn new(path: &'static str) -> Self {
        Self {
            path,
            readonly: false,
            hash: None,
        }
    }

    fn readonly(mut self, readonly: bool) -> Self {
        self.readonly = readonly;
        self
    }

    fn hash(mut self, hash: &str) -> Self {
        self.hash = Some(hash.to_string());
        self
    }

    fn build(self) -> varn::filesystem::TreeEntry {
        varn::filesystem::TreeEntry {
            path: Path::new(self.path).to_path_buf(),
            meta: varn::filesystem::EntryMeta {
                kind: varn::filesystem::EntryKind::File,
                size: 6,
                readonly: self.readonly,
                mtime: None,
                hash: self.hash,
                target: None,
                nlink: 1,
                hardlink_to: None,
                uid: None,
                gid: None,
                mode: None,
                flags: None,
                attributes: None,
                acl: None,
            },
        }
    }
}
