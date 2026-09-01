//! BUG 8 regression: restore must be transactional under file locks.
//!
//! Field report (0.2.0): restore-to-empty with a locked file deleted
//! everything else first, then died — leaving a partial tree. With
//! `--no-safety` that was unrecoverable. Fix: pre-flight probe of every
//! overwrite/delete target aborts BEFORE any change.

use crate::common::TestRepo;
use std::fs;

#[test]
fn write_protected_delete_target_is_cleared_and_restored() {
    // A write-protected file is not a lock: restore clears protection
    // before deleting (that is part of its job). The pre-flight probe
    // clears protection too, so this restore must SUCCEED.
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");

    let snapshot = repo.checkpoint("two files");

    // Agent state: a.txt deleted, b.txt write-protected.
    fs::remove_file(repo.root().join("a.txt")).unwrap();
    let b = repo.root().join("b.txt");
    fs::write(&b, b"changed").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&b, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(&b).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&b, perms).unwrap();
    }

    repo.restore(&snapshot);
    assert_eq!(repo.read_str("a.txt"), "a");
    assert_eq!(repo.read_str("b.txt"), "b");
    assert!(repo.verifies(&snapshot));

    // Cleanup for temp-dir deletion.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&b, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(&b).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&b, perms).unwrap();
    }
}

#[test]
fn write_protected_overwrite_target_is_cleared_and_restored() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"original");

    let snapshot = repo.checkpoint("two files");

    // Both files changed; b.txt write-protected.
    repo.write("a.txt", b"a changed");
    repo.write("b.txt", b"b changed");
    let b = repo.root().join("b.txt");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&b, std::fs::Permissions::from_mode(0o000)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(&b).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&b, perms).unwrap();
    }

    repo.restore(&snapshot);
    assert_eq!(repo.read_str("a.txt"), "a");
    assert_eq!(repo.read_str("b.txt"), "original");
    assert!(repo.verifies(&snapshot));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&b, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(&b).unwrap().permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&b, perms).unwrap();
    }
}

#[test]
fn clean_restore_still_works_after_pre_flight_added() {
    // Guard against the pre-flight being overly strict: a normal restore
    // with no locks must succeed.
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("sub/b.txt", b"b");

    let snapshot = repo.checkpoint("normal");

    repo.write("a.txt", b"a changed");
    fs::remove_file(repo.root().join("sub/b.txt")).unwrap();
    repo.write("stray.txt", b"stray");

    repo.restore(&snapshot);
    assert_eq!(repo.read_str("a.txt"), "a");
    assert_eq!(repo.read_str("sub/b.txt"), "b");
    assert!(!repo.root().join("stray.txt").exists());
    assert!(repo.verifies(&snapshot));
}
