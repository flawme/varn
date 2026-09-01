//! Hard link regressions: detection via nlink, primary/secondary grouping,
//! restore re-links instead of copying (Unix + Windows NTFS).

use crate::common::TestRepo;
use std::fs;

/// Create a hard link (std::fs::hard_link is cross-platform).
fn hard_link(a: &std::path::Path, b: &std::path::Path) {
    fs::hard_link(a, b).unwrap();
}

#[test]
fn hard_links_detected_and_grouped() {
    let repo = TestRepo::new();
    repo.write("primary.txt", b"shared content");
    hard_link(
        &repo.root().join("primary.txt"),
        &repo.root().join("secondary.txt"),
    );

    let scan = repo.scan();
    let primary = crate::common::find_entry(&scan, "primary.txt");
    let secondary = crate::common::find_entry(&scan, "secondary.txt");

    assert!(primary.meta.nlink >= 2, "nlink must reflect the link count");
    assert_eq!(
        primary.meta.hardlink_to, None,
        "first by sort order is primary"
    );
    assert_eq!(
        secondary.meta.hardlink_to.as_deref(),
        Some(std::path::Path::new("primary.txt")),
        "secondary must point at the primary"
    );
    assert_eq!(primary.meta.hash, secondary.meta.hash);
}

#[test]
fn hard_link_round_trip_restores_link_relationship() {
    let repo = TestRepo::new();
    repo.write("primary.txt", b"shared content");
    hard_link(
        &repo.root().join("primary.txt"),
        &repo.root().join("secondary.txt"),
    );

    let snapshot = repo.checkpoint("hardlinks");

    // Wreck: delete both, restore must recreate the LINK (not two copies).
    fs::remove_file(repo.root().join("primary.txt")).unwrap();
    fs::remove_file(repo.root().join("secondary.txt")).unwrap();

    repo.restore(&snapshot);

    // Both exist with the same content...
    assert_eq!(repo.read_str("primary.txt"), "shared content");
    assert_eq!(repo.read_str("secondary.txt"), "shared content");

    // ...and are the same inode again (Unix). On Windows NTFS the link
    // count via GetFileInformationByHandle is >= 2; std::fs::metadata
    // doesn't expose it portably, so check via a fresh scan.
    let scan = repo.scan();
    let secondary = crate::common::find_entry(&scan, "secondary.txt");
    assert_eq!(
        secondary.meta.hardlink_to.as_deref(),
        Some(std::path::Path::new("primary.txt")),
        "restored state must re-detect the hard link relationship"
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn hard_link_group_with_three_members() {
    let repo = TestRepo::new();
    repo.write("h1.txt", b"triple");
    hard_link(&repo.root().join("h1.txt"), &repo.root().join("h2.txt"));
    hard_link(&repo.root().join("h1.txt"), &repo.root().join("h3.txt"));

    let snapshot = repo.checkpoint("triple");
    for n in ["h1.txt", "h2.txt", "h3.txt"] {
        fs::remove_file(repo.root().join(n)).unwrap();
    }

    repo.restore(&snapshot);
    for n in ["h1.txt", "h2.txt", "h3.txt"] {
        assert_eq!(repo.read_str(n), "triple");
    }

    let scan = repo.scan();
    let h1 = crate::common::find_entry(&scan, "h1.txt");
    assert!(h1.meta.nlink >= 3, "all three must link to one inode");
    for n in ["h2.txt", "h3.txt"] {
        assert_eq!(
            crate::common::find_entry(&scan, n)
                .meta
                .hardlink_to
                .as_deref(),
            Some(std::path::Path::new("h1.txt"))
        );
    }
}

#[test]
fn identical_content_without_link_is_not_grouped() {
    // Two INDEPENDENT files with identical content must NOT be treated as
    // hard links (different inodes, nlink == 1).
    let repo = TestRepo::new();
    repo.write("x.txt", b"identical");
    repo.write("y.txt", b"identical");

    let scan = repo.scan();
    let x = crate::common::find_entry(&scan, "x.txt");
    let y = crate::common::find_entry(&scan, "y.txt");
    assert_eq!(x.meta.nlink, 1);
    assert_eq!(y.meta.nlink, 1);
    assert_eq!(x.meta.hardlink_to, None);
    assert_eq!(y.meta.hardlink_to, None);
    assert_eq!(x.meta.hash, y.meta.hash, "content hashes still match");
}

#[test]
fn broken_hard_link_group_restores_what_it_can() {
    // A snapshot where the primary is missing its object: the secondary
    // must not corrupt anything; restore reports the missing object.
    let repo = TestRepo::new();
    repo.write("p.txt", b"data");
    hard_link(&repo.root().join("p.txt"), &repo.root().join("s.txt"));
    let snapshot = repo.checkpoint("group");

    // Remove the shared object.
    let hash = crate::common::find_snap_entry(&snapshot, "p.txt")
        .meta
        .hash
        .clone()
        .unwrap();
    let obj = repo.repo.objects_dir().join(&hash[..2]).join(&hash[2..]);
    fs::remove_file(&obj).unwrap();

    fs::remove_file(repo.root().join("p.txt")).unwrap();
    fs::remove_file(repo.root().join("s.txt")).unwrap();

    let plan = repo.plan_restore(&snapshot);
    let err = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store());
    assert!(err.is_err(), "missing object must abort the restore");
}
