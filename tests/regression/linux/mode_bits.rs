//! Linux: full permission-mode regressions.
//!
//! The 0.2.0 parity work made restore apply the complete mode (rwx +
//! setuid/setgid/sticky), not just the readonly bit. These tests pin that.

use crate::common::{TestRepo, get_mtime, set_mtime};
use std::fs;
use std::os::unix::fs::PermissionsExt;

fn mode_of(path: &std::path::Path) -> u32 {
    fs::metadata(path).unwrap().permissions().mode() & 0o7777
}

fn set_mode(path: &std::path::Path, mode: u32) {
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(mode);
    fs::set_permissions(path, perms).unwrap();
}

#[test]
fn executable_mode_restored_exactly() {
    let repo = TestRepo::new();
    let path = repo.write("script.sh", b"#!/bin/sh\necho hi\n");
    set_mode(&path, 0o755);

    let snapshot = repo.checkpoint("exec");
    assert_eq!(
        crate::common::find_snap_entry(&snapshot, "script.sh")
            .meta
            .mode,
        Some(0o755)
    );

    set_mode(&path, 0o600);
    repo.restore(&snapshot);
    assert_eq!(mode_of(&path), 0o755, "full mode must be restored");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn restrictive_mode_restored_exactly() {
    let repo = TestRepo::new();
    let path = repo.write("secret.txt", b"secret");
    set_mode(&path, 0o400);

    let snapshot = repo.checkpoint("restrictive");
    set_mode(&path, 0o666);
    repo.restore(&snapshot);
    assert_eq!(mode_of(&path), 0o400);
    assert!(repo.verifies(&snapshot));
    set_mode(&path, 0o644); // cleanup
}

#[test]
fn setuid_setgid_sticky_restored() {
    let repo = TestRepo::new();
    let dir = repo.root().join("stickydir");
    fs::create_dir_all(&dir).unwrap();
    set_mode(&dir, 0o1777); // sticky + rwxrwxrwx

    let snapshot = repo.checkpoint("sticky");
    assert_eq!(
        crate::common::find_snap_entry(&snapshot, "stickydir")
            .meta
            .mode,
        Some(0o1777)
    );

    set_mode(&dir, 0o755);
    repo.restore(&snapshot);
    assert_eq!(mode_of(&dir), 0o1777, "sticky bit must be restored");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn group_and_other_bits_restored() {
    let repo = TestRepo::new();
    let path = repo.write("shared.txt", b"shared");
    set_mode(&path, 0o664); // rw-rw-r--

    let snapshot = repo.checkpoint("group");
    set_mode(&path, 0o600);
    repo.restore(&snapshot);
    assert_eq!(mode_of(&path), 0o664);
    assert!(repo.verifies(&snapshot));
}

#[test]
fn mode_and_mtime_restored_together() {
    let repo = TestRepo::new();
    const OLD: i64 = 1_150_000_000;
    let path = repo.write("both.txt", b"both");
    set_mode(&path, 0o750);
    set_mtime(&path, OLD);

    let snapshot = repo.checkpoint("both");
    set_mode(&path, 0o600);
    set_mtime(&path, 1_999_999_999);

    repo.restore(&snapshot);
    assert_eq!(mode_of(&path), 0o750);
    assert_eq!(get_mtime(&path), Some(OLD));
    assert!(repo.verifies(&snapshot));
}

#[test]
fn mode_drift_detected_by_diff() {
    let repo = TestRepo::new();
    let path = repo.write("f.txt", b"data");
    set_mode(&path, 0o644);
    let snapshot = repo.checkpoint("base");

    set_mode(&path, 0o600);
    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("f.txt")),
        "mode-only drift must appear in diff"
    );

    repo.restore(&snapshot);
    assert_eq!(mode_of(&path), 0o644);
}

#[test]
fn directory_mode_restored() {
    let repo = TestRepo::new();
    let dir = repo.root().join("private");
    fs::create_dir_all(&dir).unwrap();
    set_mode(&dir, 0o700);
    repo.write("private/f.txt", b"private");

    let snapshot = repo.checkpoint("private dir");
    set_mode(&dir, 0o777);
    repo.restore(&snapshot);
    assert_eq!(mode_of(&dir), 0o700, "directory mode must be restored");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn readonly_mode_fallback_for_legacy_snapshots() {
    // Snapshots from 0.1.x have mode: None; restore falls back to the
    // readonly-bit behavior.
    let repo = TestRepo::new();
    repo.write("legacy.txt", b"legacy");

    let mut entries: Vec<_> = repo.scan().entries;
    for e in entries.iter_mut() {
        if e.path == std::path::Path::new("legacy.txt") {
            e.meta.mode = None; // legacy snapshot
            e.meta.readonly = true;
        }
    }
    // Store the objects the synthetic entries reference.
    for e in &entries {
        if let Some(h) = &e.meta.hash {
            let content = fs::read(repo.repo.root.join(&e.path)).unwrap_or_default();
            repo.repo.object_store().store_content(h, &content).unwrap();
        }
    }
    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: "legacy".to_string(),
        created_at: 1,
        root: repo.repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, entries);

    fs::remove_file(repo.root().join("legacy.txt")).unwrap();
    repo.restore(&snapshot);
    assert_eq!(
        mode_of(&repo.root().join("legacy.txt")) & 0o200,
        0,
        "legacy readonly fallback must clear the owner write bit"
    );
    set_mode(&repo.root().join("legacy.txt"), 0o644);
}
