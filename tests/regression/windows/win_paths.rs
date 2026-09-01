//! Windows path-form regressions: `\\?\` prefixes, drive letters, and the
//! `C:\proj\.` display artifact (BUG 12).

use crate::common::TestRepo;

#[test]
fn init_path_display_has_no_trailing_dot_component() {
    // BUG 12: `varn init` printed `C:\...\proj\.` because absolutize
    // joined the `.` default without canonicalizing.
    let repo = TestRepo::new();
    let root_str = repo.repo.root.to_string_lossy().to_string();
    assert!(
        !root_str.ends_with("\\."),
        "root path must not carry a trailing '\\.' component: {root_str}"
    );
    assert!(
        !root_str.ends_with("/."),
        "root path must not carry a trailing '/.' component: {root_str}"
    );
}

#[test]
fn long_path_round_trip_with_extended_prefix() {
    // Windows long paths need the \\?\ prefix beyond 260 chars; the
    // scanner/restore must handle deep trees. (The report verified 902
    // chars; this pins a deep-but-CI-friendly case.)
    let repo = TestRepo::new();
    let mut rel = String::new();
    for i in 0..12 {
        rel.push_str(&format!("/level{i}"));
    }
    rel.push_str("/deep.txt");
    repo.write(&rel, b"deep windows");

    let snapshot = repo.checkpoint("deep windows");
    std::fs::remove_file(repo.root().join(&rel)).unwrap();
    repo.restore(&snapshot);
    assert_eq!(repo.read_str(&rel), "deep windows");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn backslash_forward_slash_equivalence() {
    // The scanner normalizes to forward slashes for ignore matching; the
    // snapshot paths must round-trip regardless of separator.
    let repo = TestRepo::new();
    repo.write("src/main.rs", b"fn main() {}");
    let snapshot = repo.checkpoint("seps");

    let entry = crate::common::find_snap_entry(&snapshot, "src/main.rs");
    let as_str = entry.path.to_string_lossy().replace('\\', "/");
    assert_eq!(as_str, "src/main.rs");

    std::fs::remove_file(repo.root().join("src/main.rs")).unwrap();
    repo.restore(&snapshot);
    assert_eq!(repo.read_str("src/main.rs"), "fn main() {}");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn drive_letter_absolute_paths_rejected_in_snapshots() {
    // A snapshot entry with an absolute Windows path must be rejected at
    // store time (traversal guard).
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");

    let mut entries: Vec<_> = repo.scan().entries;
    entries.push(crate::common::entry(
        "C:/evil.txt",
        varn::filesystem::EntryKind::File,
        None,
    ));

    // store_content_blobs must reject the absolute path.
    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: "evil".to_string(),
        created_at: 1,
        root: repo.repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, entries);
    let err = snapshot
        .store_content_blobs(&repo.repo.root, &repo.repo.object_store())
        .unwrap_err();
    assert!(
        err.to_string().contains("unsafe") || err.to_string().contains("path"),
        "absolute path must be rejected: {err}"
    );
}
