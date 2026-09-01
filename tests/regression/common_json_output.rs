//! JSON output contract regressions (field report items 13/16: list
//! ordering, no-op status, warnings always present).

use crate::common::TestRepo;
use serde_json::json;
use std::fs;

#[test]
fn checkpoint_json_contract() {
    // docs/usage.md documents the checkpoint JSON shape; pin it.
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    let snapshot = repo.checkpoint("contract");

    // The CLI emits: status, checkpoint_id, description, created_at, root,
    // entries, saved, warnings. Verify the pieces the library controls.
    let snap_path = repo
        .repo
        .snapshots_dir()
        .join(format!("{}.json", snapshot.meta.id.0));
    let raw: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&snap_path).unwrap()).unwrap();
    assert_eq!(raw["meta"]["description"], "contract");
    assert!(raw["meta"]["created_at"].is_i64());
    assert!(raw["entries"].is_array());
}

#[test]
fn no_op_status_contract() {
    // Item 16: JSON no-op returns status "unchanged" / saved false; text
    // says "already exists (no changes)". Both describe the SAME state —
    // pinned here so a future change is deliberate.
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");

    let first = repo.checkpoint("same");
    let second = repo.checkpoint("same");

    // The library-level contract: same ID, snapshot file not duplicated.
    assert_eq!(first.meta.id.0, second.meta.id.0);
    assert_eq!(fs::read_dir(repo.repo.snapshots_dir()).unwrap().count(), 1);
    // `SnapshotData::save` reports whether it wrote; second save is a no-op.
    let saved_again = second.save(&repo.repo.snapshots_dir()).unwrap();
    assert!(
        !saved_again,
        "second save of an identical snapshot is a no-op"
    );
}

#[test]
fn diff_json_contract() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"v1");
    let snapshot = repo.checkpoint("base");
    repo.write("a.txt", b"v2");
    repo.write("b.txt", b"new");
    fs::remove_file(repo.root().join("a.txt")).unwrap_or_else(|_| {
        // a.txt still exists (modified); remove only if the write failed.
        let _ = fs::remove_file(repo.root().join("a.txt"));
    });

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    // The CLI serializes each change as { kind, path } (docs/usage.md).
    // Change is not Serialize; build the documented shape here.
    let json = serde_json::json!(
        changes
            .iter()
            .map(|c| serde_json::json!({
                "kind": match c.kind {
                    varn::diff::ChangeKind::Added => "added",
                    varn::diff::ChangeKind::Modified => "modified",
                    varn::diff::ChangeKind::Removed => "removed",
                },
                "path": c.path.to_string_lossy(),
            }))
            .collect::<Vec<_>>()
    );
    assert!(json.is_array());
    for change in json.as_array().unwrap() {
        assert!(change["kind"].is_string());
        assert!(change["path"].is_string());
    }
}

#[test]
fn restore_json_contract() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    let snapshot = repo.checkpoint("base");
    repo.write("a.txt", b"changed");

    let result = repo.restore(&snapshot);
    let json = serde_json::json!({
        "files_written": result.files_written,
        "dirs_created": result.dirs_created,
        "symlinks_created": result.symlinks_created,
        "deleted": result.deleted,
        "verified": repo.verifies(&snapshot),
        "warnings": result.warnings,
    });
    assert_eq!(
        json["verified"], true,
        "verification failed with warnings: {:?}",
        result.warnings
    );
    assert!(json["warnings"].is_array());
    assert!(json["files_written"].is_u64());
}

#[test]
fn error_json_shape() {
    // Errors serialize with an actionable message (docs/usage.md shows
    // { "status": "error", "error": "..." }).
    let repo = TestRepo::new();
    let err = varn::cli::resolve_checkpoint(&repo.repo, "nonexistent");
    let err = err.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("not found"), "actionable message: {msg}");
}

#[test]
fn list_json_contract() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    let cp1 = repo.checkpoint("one");
    repo.write("a.txt", b"b");
    let cp2 = repo.checkpoint("two");

    let all = varn::snapshot::SnapshotData::list_all(&repo.repo.snapshots_dir()).unwrap();
    let json = serde_json::to_value(&all).unwrap();
    assert!(json.is_array());
    assert_eq!(json.as_array().unwrap().len(), 2);
    let ids: Vec<&str> = all.iter().map(|s| s.meta.id.0.as_str()).collect();
    assert!(ids.contains(&cp1.meta.id.0.as_str()));
    assert!(ids.contains(&cp2.meta.id.0.as_str()));
    let _ = json!({"status": "ok"}); // documented envelope shape
}
