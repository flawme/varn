//! Symlink round-trip regressions (scan must not follow; restore must
//! recreate; escape attempts must be refused).

use crate::common::TestRepo;
use std::fs;
use std::path::Path;

fn symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    return std::os::unix::fs::symlink(target, link);
    #[cfg(windows)]
    {
        if target.is_dir() {
            std::os::windows::fs::symlink_dir(target, link)
        } else {
            std::os::windows::fs::symlink_file(target, link)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (target, link);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "symlinks unsupported",
        ))
    }
}

#[test]
fn symlink_scanned_as_symlink_not_followed() {
    let repo = TestRepo::new();
    repo.write("target.txt", b"target content");
    symlink(
        &std::path::PathBuf::from("target.txt"),
        &repo.root().join("link.txt"),
    )
    .unwrap();

    let scan = repo.scan();
    let link = crate::common::find_entry(&scan, "link.txt");
    assert_eq!(link.meta.kind, varn::filesystem::EntryKind::Symlink);
    assert!(link.meta.hash.is_none(), "symlinks are not content-hashed");
    assert_eq!(
        link.meta.target.as_ref().unwrap(),
        &std::path::PathBuf::from("target.txt")
    );
}

#[test]
fn symlink_round_trip() {
    let repo = TestRepo::new();
    repo.write("target.txt", b"target content");
    symlink(
        &std::path::PathBuf::from("target.txt"),
        &repo.root().join("link.txt"),
    )
    .unwrap();

    let snapshot = repo.checkpoint("symlink");
    fs::remove_file(repo.root().join("link.txt")).unwrap();
    repo.restore(&snapshot);

    let link = repo.root().join("link.txt");
    assert!(link.is_symlink());
    assert_eq!(
        fs::read_link(&link).unwrap(),
        std::path::PathBuf::from("target.txt")
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn dangling_symlink_round_trip() {
    let repo = TestRepo::new();
    symlink(
        &repo.root().join("does-not-exist.txt"),
        &repo.root().join("dangling.txt"),
    )
    .unwrap();

    let snapshot = repo.checkpoint("dangling");
    fs::remove_file(repo.root().join("dangling.txt")).unwrap();
    repo.restore(&snapshot);

    let link = repo.root().join("dangling.txt");
    assert!(link.is_symlink());
    assert!(!link.exists(), "dangling stays dangling");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn symlink_to_directory_round_trip() {
    let repo = TestRepo::new();
    fs::create_dir_all(repo.root().join("realdir")).unwrap();
    repo.write("realdir/f.txt", b"inside");
    // Windows: directory symlinks require Developer Mode or admin. When
    // the privilege is missing, skip (junction coverage lives in the
    // Windows-specific suite).
    if symlink(&repo.root().join("realdir"), &repo.root().join("dirlink")).is_err() {
        eprintln!("skipping: directory symlinks require privileges on this platform");
        return;
    }

    let snapshot = repo.checkpoint("dir link");
    // Removing a directory symlink differs by platform: Unix remove_file
    // unlinks the link itself; Windows directory symlinks are reparse
    // points that remove_file rejects with Access Denied — remove_dir
    // removes the link (not the target).
    let dirlink = repo.root().join("dirlink");
    #[cfg(unix)]
    fs::remove_file(&dirlink).unwrap();
    #[cfg(windows)]
    fs::remove_dir(&dirlink).unwrap();
    repo.restore(&snapshot);

    assert!(repo.root().join("dirlink").is_symlink());
    // The realdir CONTENTS must be captured exactly once (through the real
    // path, not through the link).
    assert_eq!(repo.read_str("realdir/f.txt"), "inside");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn symlink_escape_in_leading_path_is_refused() {
    // The leading-path check is defense in depth against plans whose
    // surrounding layout changed between plan and execute (e.g. a hostile
    // or stale snapshot JSON, or another process swapping a directory for
    // a symlink mid-restore). Build such a plan directly: WriteFile into
    // `sub/` while `sub` is a symlink to outside the root, with NO plan
    // action that replaces the symlink.
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    let snapshot = repo.checkpoint("escape");

    // Layout: sub is a symlink to a directory OUTSIDE the root.
    let outside = std::env::temp_dir().join("varn-escape-target");
    fs::create_dir_all(&outside).unwrap();
    symlink(&outside, &repo.root().join("sub")).unwrap();

    // A plan that writes sub/f.txt without touching the symlink itself.
    let hash = varn::filesystem::hash_bytes(b"payload");
    repo.repo
        .object_store()
        .store_content(&hash, b"payload")
        .unwrap();
    let plan = varn::restore::RestorePlan {
        actions: vec![varn::restore::RestoreAction::WriteFile {
            path: std::path::PathBuf::from("sub/f.txt"),
            hash: hash.clone(),
            readonly: false,
            mtime: None,
            uid: None,
            gid: None,
            mode: None,
            flags: None,
            attributes: None,
            acl: None,
        }],
        conflicts: vec![],
        warnings: vec![],
    };

    let err = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store());
    assert!(
        err.is_err(),
        "restore must refuse to write through a symlink in the leading path"
    );
    assert!(
        !outside.join("f.txt").exists(),
        "the payload must NOT have been written outside the root"
    );
    let _ = snapshot;
}

#[test]
fn symlink_target_change_is_detected() {
    let repo = TestRepo::new();
    repo.write("t1.txt", b"one");
    repo.write("t2.txt", b"two");
    symlink(
        &std::path::PathBuf::from("t1.txt"),
        &repo.root().join("link"),
    )
    .unwrap();

    let snapshot = repo.checkpoint("links");

    // Retarget the link.
    fs::remove_file(repo.root().join("link")).unwrap();
    symlink(
        &std::path::PathBuf::from("t2.txt"),
        &repo.root().join("link"),
    )
    .unwrap();

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("link")),
        "retargeted symlink must appear in diff"
    );

    repo.restore(&snapshot);
    assert_eq!(
        fs::read_link(repo.root().join("link")).unwrap(),
        std::path::PathBuf::from("t1.txt")
    );
    assert!(repo.verifies(&snapshot));
}
