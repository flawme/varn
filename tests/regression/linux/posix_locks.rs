//! Linux: POSIX lock/permission regressions (the Linux analog of the
//! Windows sharing-violation BUG 8).

use crate::common::TestRepo;
use std::fs;
use std::os::unix::fs::PermissionsExt;

#[test]
fn unwritable_file_aborts_restore_before_changes() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");
    let snapshot = repo.checkpoint("two");

    // a.txt deleted, b.txt made unwritable (mode 000).
    fs::remove_file(repo.root().join("a.txt")).unwrap();
    let b = repo.root().join("b.txt");
    fs::write(&b, b"changed").unwrap();
    fs::set_permissions(&b, fs::Permissions::from_mode(0o000)).unwrap();

    let plan = repo.plan_restore(&snapshot);
    let result = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store());
    // Running as root, mode 000 is still writable — the pre-flight passes
    // and the restore succeeds. As a normal user it must abort BEFORE
    // restoring a.txt. Both outcomes are valid; the invariant is that a
    // failed restore leaves a.txt untouched.
    if result.is_err() {
        assert!(
            !repo.root().join("a.txt").exists(),
            "aborted restore must not have restored a.txt"
        );
    } else {
        assert_eq!(repo.read_str("a.txt"), "a");
        assert_eq!(repo.read_str("b.txt"), "b");
    }
    fs::set_permissions(&b, fs::Permissions::from_mode(0o644)).unwrap();
}

#[test]
fn undeletable_file_in_readonly_dir_reported() {
    let repo = TestRepo::new();
    repo.write("keep.txt", b"keep");
    // rodir is part of the checkpoint (so restore keeps the directory and
    // only deletes the stray inside it).
    repo.write("rodir/keep.txt", b"inner");
    let snapshot = repo.checkpoint("clean");

    // A stray file in a read-only directory cannot be deleted by a
    // non-root user (deletion requires write permission on the dir).
    fs::write(repo.root().join("rodir/stray.txt"), b"stray").unwrap();
    fs::set_permissions(
        repo.root().join("rodir").as_path(),
        fs::Permissions::from_mode(0o555),
    )
    .unwrap();

    let plan = repo.plan_restore(&snapshot);
    let result = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store());

    if unsafe { geteuid() } != 0 {
        assert!(
            result.is_err(),
            "non-root must fail to delete from a read-only directory"
        );
        assert_eq!(repo.read_str("keep.txt"), "keep");
    }
    fs::set_permissions(
        repo.root().join("rodir").as_path(),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
}

unsafe extern "C" {
    fn geteuid() -> u32;
}

#[test]
fn sticky_sgid_directory_restore() {
    let repo = TestRepo::new();
    let dir = repo.root().join("sgid");
    fs::create_dir_all(&dir).unwrap();
    let mut perms = fs::metadata(&dir).unwrap().permissions();
    perms.set_mode(0o2770); // setgid + rwxrwx---
    fs::set_permissions(&dir, perms).unwrap();

    let snapshot = repo.checkpoint("sgid");
    let mut perms = fs::metadata(&dir).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&dir, perms).unwrap();

    repo.restore(&snapshot);
    let mode = fs::metadata(&dir).unwrap().permissions().mode() & 0o7777;
    assert_eq!(mode, 0o2770, "setgid bit must be restored on directories");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn unreadable_file_checkpoint_then_readable_restore() {
    // The BUG 5 scenario on Linux: unreadable at checkpoint, readable at
    // restore. The checkpoint must not poison; the later restore must work
    // for everything else.
    let repo = TestRepo::new();
    repo.write("ok.txt", b"ok");
    let locked = repo.write("locked.txt", b"secret");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();

    let snapshot = repo.checkpoint("with locked");

    // Make it readable again and delete both; restore must bring back
    // ok.txt. The locked file was intentionally omitted from the snapshot.
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(&locked).unwrap();
    fs::remove_file(repo.root().join("ok.txt")).unwrap();

    let result = repo.restore(&snapshot);
    assert_eq!(repo.read_str("ok.txt"), "ok");
    assert!(
        !repo.root().join("locked.txt").exists(),
        "an unreadable file must not be represented as restorable content"
    );
    assert!(
        result.warnings.is_empty(),
        "unexpected restore warnings: {:?}",
        result.warnings
    );
}
