//! BUG 4 regression: checkpoint IDs must be deterministic.
//!
//! Field report (0.2.0): the ID hashed `created_at`, so checkpointing an
//! unchanged state twice produced two IDs — silently breaking the
//! documented "same state twice is a no-op" contract. Metadata-only changes
//! (mode/attributes) were also invisible to the ID, so the second
//! checkpoint was dropped entirely.

use crate::common::TestRepo;
use varn::filesystem::EntryKind;

#[test]
fn same_state_same_description_produces_same_id() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"content");
    repo.write("src/main.rs", b"fn main() {}");

    let cp1 = repo.checkpoint("same desc");
    let cp2 = repo.checkpoint("same desc");

    assert_eq!(
        cp1.meta.id.0, cp2.meta.id.0,
        "unchanged state + same description must produce the same ID"
    );
}

#[test]
fn same_state_different_description_produces_different_id() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"content");

    let cp1 = repo.checkpoint("desc one");
    let cp2 = repo.checkpoint("desc two");

    assert_ne!(cp1.meta.id.0, cp2.meta.id.0);
}

#[test]
fn content_change_produces_different_id() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"v1");
    let cp1 = repo.checkpoint("desc");

    repo.write("a.txt", b"v2");
    let cp2 = repo.checkpoint("desc");

    assert_ne!(cp1.meta.id.0, cp2.meta.id.0);
}

#[test]
fn metadata_only_change_produces_different_id() {
    let repo = TestRepo::new();
    let path = repo.write("script.sh", b"#!/bin/sh\necho hi\n");

    let cp1 = repo.checkpoint("desc");

    // Change only the permission mode (Unix) / readonly attribute (all).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    #[cfg(not(unix))]
    {
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&path, perms).unwrap();
    }

    let cp2 = repo.checkpoint("desc");
    assert_ne!(
        cp1.meta.id.0, cp2.meta.id.0,
        "a metadata-only change must produce a new checkpoint, not be \
         silently dropped as a no-op"
    );
}

#[test]
fn id_is_independent_of_checkpoint_time() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"content");

    // Two snapshots of the same state with different created_at values
    // must produce the same ID (the ID is a pure function of state).
    let scan = repo.scan();
    let mk = |created_at: i64| {
        let meta = varn::core::CheckpointMeta {
            id: varn::core::CheckpointId("pending".to_string()),
            description: "desc".to_string(),
            created_at,
            root: repo.repo.root.clone(),
        };
        varn::snapshot::SnapshotData::new(meta, scan.entries.clone())
    };
    let s1 = mk(1_000_000);
    let s2 = mk(2_000_000);
    assert_ne!(s1.meta.created_at, s2.meta.created_at);
    assert_eq!(
        s1.meta.id.0, s2.meta.id.0,
        "created_at must not influence the checkpoint ID"
    );
}

#[test]
fn id_changes_when_hardlink_relationship_changes() {
    let repo = TestRepo::new();
    repo.write("primary.txt", b"shared");
    repo.write("secondary.txt", b"shared");

    let scan = repo.scan();
    let mk = |with_link: bool| {
        let mut entries = scan.entries.clone();
        if with_link {
            for e in entries.iter_mut() {
                if e.path == std::path::Path::new("secondary.txt") {
                    e.meta.hardlink_to = Some(std::path::PathBuf::from("primary.txt"));
                }
            }
        }
        let meta = varn::core::CheckpointMeta {
            id: varn::core::CheckpointId("pending".to_string()),
            description: "desc".to_string(),
            created_at: 1,
            root: repo.repo.root.clone(),
        };
        varn::snapshot::SnapshotData::new(meta, entries)
    };
    let without = mk(false);
    let with = mk(true);
    assert_ne!(
        without.meta.id.0, with.meta.id.0,
        "hardlink relationship is part of the state and must affect the ID"
    );
}

#[test]
fn entries_sorted_before_id_hash() {
    // The ID must not depend on the order entries are handed in; the same
    // set of entries in different orders must produce the same ID.
    let repo = TestRepo::new();
    repo.write("a.txt", b"a");
    repo.write("b.txt", b"b");
    repo.write("c.txt", b"c");
    let scan = repo.scan();

    let mk = |entries: Vec<varn::filesystem::TreeEntry>| {
        let meta = varn::core::CheckpointMeta {
            id: varn::core::CheckpointId("pending".to_string()),
            description: "desc".to_string(),
            created_at: 1,
            root: repo.repo.root.clone(),
        };
        varn::snapshot::SnapshotData::new(meta, entries)
    };

    let forward = mk(scan.entries.clone());
    let mut reversed = scan.entries.clone();
    reversed.reverse();
    let backward = mk(reversed);

    assert_eq!(forward.meta.id.0, backward.meta.id.0);
    // And SnapshotData::new must have sorted both.
    let sorted: Vec<_> = forward
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();
    let mut expected = sorted.clone();
    expected.sort();
    assert_eq!(sorted, expected);
    let _ = EntryKind::File; // keep import used on all platforms
}
