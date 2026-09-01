//! Safety-checkpoint regressions: the pre-restore snapshot of current
//! state, and recovery from a bad restore.

use crate::common::TestRepo;
use std::fs;

#[test]
fn safety_checkpoint_captures_pre_restore_state() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"original");
    let target = repo.checkpoint("target");

    // Change state after the checkpoint.
    repo.write("a.txt", b"modified");
    repo.write("extra.txt", b"extra");

    // Restore (the CLI creates a safety checkpoint first; at the API level
    // the caller does — replicate that flow).
    let safety = repo.checkpoint("safety before restore");
    repo.restore(&target);

    // The safety checkpoint can restore the pre-restore state.
    repo.restore(&safety);
    assert_eq!(repo.read_str("a.txt"), "modified");
    assert_eq!(repo.read_str("extra.txt"), "extra");
}

#[test]
fn recovery_from_bad_restore() {
    let repo = TestRepo::new();
    repo.write("good.txt", b"good state");
    let safety = repo.checkpoint("safety");

    // "Bad" state gets checkpointed as a target, then restored.
    repo.write("good.txt", b"corrupted");
    repo.write("junk.txt", b"junk");
    let bad = repo.checkpoint("bad state");
    repo.restore(&bad);
    assert_eq!(repo.read_str("good.txt"), "corrupted");
    assert!(repo.root().join("junk.txt").exists());

    // Recover via the safety checkpoint.
    repo.restore(&safety);
    assert_eq!(repo.read_str("good.txt"), "good state");
    assert!(!repo.root().join("junk.txt").exists());
    assert!(repo.verifies(&safety));
}

#[test]
fn safety_checkpoint_of_state_with_readonly_files() {
    // BUG 3 interaction: the safety checkpoint itself may contain read-only
    // files; restoring FROM it must work (clear-protect fix).
    let repo = TestRepo::new();
    let path = repo.write("ro.txt", b"readonly");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();
    }

    let safety = repo.checkpoint("safety with ro");

    // Drift, then restore the safety checkpoint twice (the second restore
    // overwrites a read-only file). Clear protection to simulate the drift
    // (a real writer would have write access).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&path, perms).unwrap();
    }
    repo.write("ro.txt", b"drifted");
    repo.restore(&safety);
    repo.restore(&safety);
    assert_eq!(repo.read_str("ro.txt"), "readonly");
    assert!(repo.verifies(&safety));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = fs::metadata(&path).unwrap().permissions();
        #[allow(clippy::set_readonly_false)]
        perms.set_readonly(false);
        fs::set_permissions(&path, perms).unwrap();
    }
}

#[test]
fn restore_to_empty_tree_via_safety() {
    let repo = TestRepo::new();
    // Checkpoint an empty state (only the root dir entry).
    let scan = repo.scan();
    assert!(
        scan.entries
            .iter()
            .all(|e| e.meta.kind != varn::filesystem::EntryKind::File)
    );

    repo.write("f.txt", b"data");
    let empty = repo.checkpoint_from_scan(&scan, "empty");

    // Restore to empty: f.txt must be deleted (as a conflict).
    repo.restore(&empty);
    assert!(!repo.root().join("f.txt").exists());
    assert!(repo.verifies(&empty));
}

#[test]
fn repeated_restores_are_idempotent() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("sub/b.txt", b"b");
    let target = repo.checkpoint("target");

    repo.write("a.txt", b"changed");
    repo.write("extra.txt", b"extra");

    for i in 0..3 {
        repo.restore(&target);
        assert_eq!(repo.read_str("a.txt"), "a", "iteration {i}");
        assert_eq!(repo.read_str("sub/b.txt"), "b", "iteration {i}");
        assert!(!repo.root().join("extra.txt").exists(), "iteration {i}");
        assert!(repo.verifies(&target), "iteration {i}");
    }
}
