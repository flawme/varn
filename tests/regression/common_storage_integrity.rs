//! Storage-layer integrity regressions: content-addressing, dedup,
//! corruption detection, atomicity.

use crate::common::TestRepo;

#[test]
fn identical_content_stored_once() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"same bytes");
    repo.write("sub/b.txt", b"same bytes");

    let snapshot = repo.checkpoint("dedup");
    let hashes: Vec<_> = snapshot
        .entries
        .iter()
        .filter_map(|e| e.meta.hash.clone())
        .collect();
    assert_eq!(hashes[0], hashes[1], "identical content must share a hash");

    // The store holds exactly one object for both files.
    let store_count = snapshot.referenced_hashes().len();
    assert_eq!(store_count, 1);
}

#[test]
fn corrupted_object_is_detected_before_overwrite() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"original");
    let snapshot = repo.checkpoint("integrity");

    // Tamper with the stored object.
    let hash = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("a.txt"))
        .unwrap()
        .meta
        .hash
        .clone()
        .unwrap();
    let obj_path = repo.repo.objects_dir().join(&hash[..2]).join(&hash[2..]);
    std::fs::write(&obj_path, b"tampered!!").unwrap();

    // Restore must refuse (hash mismatch) rather than write corrupt data.
    repo.write("a.txt", b"changed");
    let plan = repo.plan_restore(&snapshot);
    let err = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store())
        .unwrap_err();
    assert!(
        err.to_string().contains("mismatch") || err.to_string().contains("corrupt"),
        "expected a corruption error, got: {err}"
    );
}

#[test]
fn missing_object_aborts_before_any_writes() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");
    let snapshot = repo.checkpoint("two");

    // Remove one object.
    let hash_b = snapshot
        .entries
        .iter()
        .find(|e| e.path == std::path::Path::new("b.txt"))
        .unwrap()
        .meta
        .hash
        .clone()
        .unwrap();
    let obj = repo
        .repo
        .objects_dir()
        .join(&hash_b[..2])
        .join(&hash_b[2..]);
    std::fs::remove_file(&obj).unwrap();

    // Both files changed; restore must abort before writing either.
    repo.write("a.txt", b"a changed");
    repo.write("b.txt", b"b changed");

    let plan = repo.plan_restore(&snapshot);
    let err = varn::restore::execute_restore(&plan, &repo.repo.root, &repo.repo.object_store())
        .unwrap_err();
    assert!(
        err.to_string().contains("missing object"),
        "expected missing-object abort, got: {err}"
    );
    assert_eq!(
        repo.read_str("a.txt"),
        "a changed",
        "no file may be written when an object is missing"
    );
}

#[test]
fn snapshot_files_are_valid_json_and_round_trip() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("sub/b.txt", b"b");
    let snapshot = repo.checkpoint("roundtrip");

    let path = repo
        .repo
        .snapshots_dir()
        .join(format!("{}.json", snapshot.meta.id.0));
    let raw = std::fs::read_to_string(&path).unwrap();
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).expect("snapshot must be valid JSON");

    let loaded = varn::snapshot::SnapshotData::load(&path).unwrap();
    assert_eq!(loaded.meta.id.0, snapshot.meta.id.0);
    assert_eq!(loaded.entries.len(), snapshot.entries.len());
    let _ = parsed;
}

#[test]
fn snapshot_id_rejects_path_traversal() {
    let repo = TestRepo::new();
    let snapshots_dir = repo.repo.snapshots_dir();
    assert!(varn::snapshot::SnapshotData::load(&snapshots_dir.join("../../../etc.json")).is_err());
    assert!(varn::snapshot::SnapshotData::load(&snapshots_dir.join("..\\..\\x.json")).is_err());
}

#[test]
fn object_store_sharding_is_stable() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"shard me");
    let snapshot = repo.checkpoint("shard");
    let hash = snapshot.entries[0].meta.hash.clone().unwrap();

    // shard = first 2 chars, object = remaining chars.
    let expected = repo.repo.objects_dir().join(&hash[..2]).join(&hash[2..]);
    assert!(expected.is_file(), "object must be at objects/<aa>/<rest>");
}

#[test]
fn concurrent_checkpoints_do_not_tear_snapshots() {
    // The report verified atomic rename prevents torn snapshots; pin it.
    use std::sync::Arc;
    let repo = Arc::new(TestRepo::new());
    repo.write("f.txt", b"concurrent");

    // All workers use the SAME description so the deterministic ID must
    // be identical; concurrent saves of the same snapshot must not tear.
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let repo = Arc::clone(&repo);
            std::thread::spawn(move || repo.checkpoint("concurrent"))
        })
        .collect();
    let mut ids = Vec::new();
    for h in handles {
        ids.push(h.join().unwrap().meta.id.0);
    }
    let unique: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(
        unique.len(),
        1,
        "identical state + description must yield identical IDs; got {ids:?}"
    );
    let count = std::fs::read_dir(repo.repo.snapshots_dir())
        .unwrap()
        .count();
    assert_eq!(count, 1, "no duplicate snapshot files");
}
