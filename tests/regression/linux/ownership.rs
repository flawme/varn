//! Linux: uid/gid ownership regressions.
//!
//! Ownership restore is best-effort (requires root for other users); these
//! tests pin the same-user round trip and the warning-not-failure behavior.

use crate::common::TestRepo;
use std::fs;
use std::os::unix::fs::MetadataExt;

#[test]
fn ownership_captured_for_current_user() {
    let repo = TestRepo::new();
    repo.write("owned.txt", b"mine");

    let snapshot = repo.checkpoint("owned");
    let entry = crate::common::find_snap_entry(&snapshot, "owned.txt");
    // Running as a normal user: uid/gid are the process's own ids.
    assert!(entry.meta.uid.is_some(), "uid must be captured on Linux");
    assert!(entry.meta.gid.is_some(), "gid must be captured on Linux");
}

#[test]
fn ownership_restore_same_user_succeeds() {
    let repo = TestRepo::new();
    repo.write("owned.txt", b"mine");
    let snapshot = repo.checkpoint("owned");

    fs::remove_file(repo.root().join("owned.txt")).unwrap();
    let result = repo.restore(&snapshot);

    // Restoring to the same uid/gid must not warn (chown to self works).
    assert!(
        result.warnings.iter().all(|w| !w.contains("ownership")),
        "same-user ownership restore must not warn: {:?}",
        result.warnings
    );
    let meta = fs::metadata(repo.root().join("owned.txt")).unwrap();
    let expected_uid = std::os::unix::fs::MetadataExt::uid(&fs::metadata(&repo.repo.root).unwrap());
    assert_eq!(meta.uid(), expected_uid);
    assert!(repo.verifies(&snapshot));
}

#[test]
fn ownership_failure_is_warning_not_error() {
    // Simulate an un-restorable ownership: craft a snapshot entry with a
    // uid/gid the process cannot chown to (a high uid nobody owns).
    let repo = TestRepo::new();
    repo.write("f.txt", b"data");

    let mut entries: Vec<_> = repo.scan().entries;
    for e in entries.iter_mut() {
        if e.path == std::path::Path::new("f.txt") {
            e.meta.uid = Some(54321); // almost certainly not ours
            e.meta.gid = Some(54321);
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
        description: "foreign owner".to_string(),
        created_at: 1,
        root: repo.repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, entries);

    fs::remove_file(repo.root().join("f.txt")).unwrap();

    // Running as root (CI) this succeeds silently; as a normal user it
    // warns. Either way the restore must SUCCEED and the file must exist.
    let result = repo.restore(&snapshot);
    assert!(repo.root().join("f.txt").is_file());
    assert!(repo.verifies(&snapshot));
    if libc_geteuid() != 0 {
        assert!(
            result.warnings.iter().any(|w| w.contains("ownership")),
            "non-root must see an ownership warning: {:?}",
            result.warnings
        );
    }
}

// libc without the libc crate: read /proc/self or use std? std has no geteuid.
// Use a tiny extern (same pattern as platform.rs lchflags).
unsafe extern "C" {
    fn geteuid() -> u32;
}

fn libc_geteuid() -> u32 {
    unsafe { geteuid() }
}

#[test]
fn ownership_of_directories_captured() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.root().join("dir")).unwrap();
    repo.write("dir/f.txt", b"x");

    let snapshot = repo.checkpoint("dir owned");
    let dir_entry = crate::common::find_snap_entry(&snapshot, "dir");
    assert!(dir_entry.meta.uid.is_some());
    assert!(dir_entry.meta.gid.is_some());
}
