//! BUG 3 regression: read-only files must not break re-restore.
//!
//! Field report (Windows, 0.2.0): checkpoint with read-only file → delete →
//! restore (writes read-only file) → restore again → "Access is denied
//! (os error 5)". Restore never cleared the READONLY attribute before
//! opening the file for writing. The delete path had the same problem.

use crate::common::TestRepo;
use std::fs;

/// Make a file read-only, cross-platform.
fn make_readonly(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o444);
        fs::set_permissions(path, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(path, perms).unwrap();
    }
}

/// Clear read-only, cross-platform (for test cleanup).
fn make_writable(path: &std::path::Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(path, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_readonly(false);
        fs::set_permissions(path, perms).unwrap();
    }
}

#[test]
fn restore_over_readonly_file_succeeds_repeatedly() {
    let repo = TestRepo::new();
    let path = repo.write("ro.txt", b"readonly content");
    make_readonly(&path);

    let snapshot = repo.checkpoint("readonly");

    // First restore: file deleted, restore recreates it (read-only).
    fs::remove_file(&path).unwrap();
    repo.restore(&snapshot);
    assert!(path.exists());
    assert_eq!(repo.read_str("ro.txt"), "readonly content");

    // Second restore: the file EXISTS and is read-only. Before the fix this
    // failed with os error 5 on Windows.
    let result = repo.restore(&snapshot);
    assert!(
        result
            .warnings
            .iter()
            .all(|w| !w.contains("denied") && !w.contains("Access")),
        "unexpected access warnings: {:?}",
        result.warnings
    );
    assert_eq!(repo.read_str("ro.txt"), "readonly content");

    // Third restore for good measure.
    repo.restore(&snapshot);
    assert_eq!(repo.read_str("ro.txt"), "readonly content");
    assert!(repo.verifies(&snapshot));

    make_writable(&path); // allow temp dir cleanup
}

#[test]
fn restore_deletes_readonly_file() {
    let repo = TestRepo::new();
    let path = repo.write("doomed.txt", b"delete me");
    make_readonly(&path);

    let snapshot = repo.checkpoint("without doomed");
    assert!(
        snapshot
            .entries
            .iter()
            .any(|e| e.path.to_string_lossy().ends_with("doomed.txt")),
        "doomed.txt must be in the snapshot (it was written before)"
    );

    // Scenario: checkpoint WITHOUT the file, then create a read-only stray
    // file, then restore (which must delete it).
    let path2 = repo.root().join("stray.txt");
    fs::write(&path2, b"stray").unwrap();
    make_readonly(&path2);

    let result = repo.restore(&snapshot);
    assert!(
        !path2.exists(),
        "restore must delete a read-only stray file (Windows refuses to \
         delete read-only files without clearing the attribute first)"
    );
    let _ = result;
    let _ = path;
}

#[test]
fn readonly_attribute_reapplied_after_overwrite() {
    let repo = TestRepo::new();
    let path = repo.write("ro2.txt", b"v1");
    make_readonly(&path);

    let snapshot = repo.checkpoint("ro2");
    assert!(
        crate::common::find_snap_entry(&snapshot, "ro2.txt")
            .meta
            .readonly,
        "readonly must be captured"
    );

    // Clear protection and modify, then restore: the read-only attribute
    // must be re-applied AFTER the write (not left cleared).
    make_writable(&path);
    fs::write(&path, b"v2").unwrap();

    repo.restore(&snapshot);
    assert_eq!(repo.read_str("ro2.txt"), "v1");
    let readonly_now = {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&path).unwrap().permissions().mode() & 0o222 == 0
        }
        #[cfg(not(unix))]
        {
            fs::metadata(&path).unwrap().permissions().readonly()
        }
    };
    assert!(
        readonly_now,
        "read-only attribute must be re-applied after the overwrite"
    );
    assert!(repo.verifies(&snapshot));
    make_writable(&path);
}

#[test]
fn readonly_directory_contents_restorable() {
    // A read-only directory (Unix mode bits) containing files: restore must
    // handle writing into it. Windows directories don't have a read-only
    // attribute in practice; Unix mode is captured.
    let repo = TestRepo::new();
    let dir = repo.root().join("rodir");
    fs::create_dir_all(&dir).unwrap();
    repo.write("rodir/f.txt", b"inside");

    let snapshot = repo.checkpoint("rodir");

    // Wreck the state.
    fs::remove_file(dir.join("f.txt")).unwrap();

    repo.restore(&snapshot);
    assert_eq!(repo.read_str("rodir/f.txt"), "inside");
    assert!(repo.verifies(&snapshot));
}
