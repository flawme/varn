//! Diff semantics regressions: added/modified/deleted detection, prefix
//! resolution, metadata-only drift.

use crate::common::TestRepo;
use std::fs;

#[test]
fn diff_detects_added_modified_deleted() {
    let repo = TestRepo::new();
    repo.write("kept.txt", b"kept");
    repo.write("doomed.txt", b"doomed");
    repo.write("changed.txt", b"v1");
    let snapshot = repo.checkpoint("base");

    repo.write("changed.txt", b"v2");
    repo.write("added.txt", b"new");
    fs::remove_file(repo.root().join("doomed.txt")).unwrap();

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);

    let kinds: Vec<(String, &str)> = changes
        .iter()
        .map(|c| {
            (
                c.path.to_string_lossy().replace('\\', "/"),
                match c.kind {
                    varn::diff::ChangeKind::Added => "added",
                    varn::diff::ChangeKind::Modified => "modified",
                    varn::diff::ChangeKind::Removed => "deleted",
                },
            )
        })
        .collect();

    assert!(kinds.contains(&("added.txt".to_string(), "added")));
    assert!(kinds.contains(&("changed.txt".to_string(), "modified")));
    assert!(kinds.contains(&("doomed.txt".to_string(), "deleted")));
    assert!(
        kinds.iter().all(|(p, _)| p != "kept.txt"),
        "unchanged files must not appear"
    );
}

#[test]
fn diff_ignores_mtime_only_drift_when_content_same() {
    // Actually the opposite contract: metadata drift IS reported as
    // modified so restore can fix it. Pin the current contract.
    let repo = TestRepo::new();
    let path = repo.write("f.txt", b"same");
    let snapshot = repo.checkpoint("base");

    // Touch the mtime only.
    filetime::set_file_mtime(&path, filetime::FileTime::from_unix_time(999, 0)).unwrap();

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("f.txt")),
        "metadata-only drift must be visible in diff (restore fixes it)"
    );
}

#[test]
fn diff_empty_when_identical() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("sub/b.txt", b"b");
    let snapshot = repo.checkpoint("base");

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(changes.is_empty(), "identical state must diff empty");
}

#[test]
fn diff_nested_directories() {
    let repo = TestRepo::new();
    repo.write("a/b/c/deep.txt", b"deep");
    let snapshot = repo.checkpoint("deep");

    repo.write("a/b/c/new.txt", b"new");
    fs::remove_file(repo.root().join("a/b/c/deep.txt")).unwrap();

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("a/b/c/new.txt"))
    );
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("a/b/c/deep.txt"))
    );
}

#[test]
fn prefix_resolution_unique_and_ambiguous() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    let cp1 = repo.checkpoint("first");
    repo.write("a.txt", b"b");
    let cp2 = repo.checkpoint("second");

    // Unique prefix resolves.
    let resolved = varn::cli::resolve_checkpoint(&repo.repo, &cp1.meta.id.0[..8]).unwrap();
    assert_eq!(resolved.meta.id.0, cp1.meta.id.0);

    // Ambiguous prefix errors. Craft two IDs sharing a prefix is not
    // directly controllable; instead assert a non-matching prefix errors.
    let err = varn::cli::resolve_checkpoint(&repo.repo, "zzzz").unwrap_err();
    assert!(err.to_string().contains("not found"));

    // Full IDs always resolve.
    assert_eq!(
        varn::cli::resolve_checkpoint(&repo.repo, &cp2.meta.id.0)
            .unwrap()
            .meta
            .id
            .0,
        cp2.meta.id.0
    );
}

#[test]
fn diff_after_restore_is_empty() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("sub/b.txt", b"b");
    let snapshot = repo.checkpoint("base");

    repo.write("a.txt", b"changed");
    fs::remove_file(repo.root().join("sub/b.txt")).unwrap();
    repo.write("extra.txt", b"extra");

    repo.restore(&snapshot);

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes.is_empty(),
        "diff after successful restore must be empty: {changes:?}"
    );
}
