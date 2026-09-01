//! BUG 8 regression: restore must be transactional under file locks.
//!
//! Field report (0.2.0): restore-to-empty with a locked file deleted
//! everything else first, then died — leaving a partial tree. With
//! `--no-safety` that was unrecoverable. Fix: pre-flight probe of every
//! overwrite/delete target aborts BEFORE any change.

use crate::common::TestRepo;
use std::fs;

#[test]
fn locked_delete_target_aborts_before_any_changes() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");

    let snapshot = repo.checkpoint("two files");

    // Simulate the agent state: a.txt deleted, b.txt made undeletable.
    fs::remove_file(repo.root().join("a.txt")).unwrap();
    let b = repo.root().join("b.txt");
    fs::write(&b, b"changed").unwrap();

    // Make b.txt undeletable the portable way: on Windows the readonly
    // attribute blocks deletion; on Unix a sticky/read-only parent is not
    // portable, so use the readonly file attribute path (restore's delete
    // path clears it — but the PRE-FLIGHT probe must fire first for a
    // genuinely locked file). For a portable probe test, make b.txt
    // unreadable-unwritable on Unix; the write-probe fails there.
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

    // The restore must FAIL (pre-flight) — and a.txt must still be absent
    // (no partial application: the plan wanted to restore a.txt AND delete
    // b.txt; the abort must happen before a.txt is written).
    let plan = repo.plan_restore(&snapshot);
    let err = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store())
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("pre-flight") || msg.contains("not writable") || msg.contains("Access"),
        "expected a pre-flight failure, got: {msg}"
    );
    assert!(
        !repo.root().join("a.txt").exists(),
        "pre-flight abort must happen BEFORE any filesystem change"
    );

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
fn locked_overwrite_target_aborts_before_any_changes() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"original");

    let snapshot = repo.checkpoint("two files");

    // Both files changed; b.txt is unwritable (lock analog).
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

    let plan = repo.plan_restore(&snapshot);
    let result = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store());
    assert!(result.is_err(), "pre-flight must reject the locked target");

    // a.txt must be untouched by the aborted restore.
    assert_eq!(repo.read_str("a.txt"), "a changed");

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
