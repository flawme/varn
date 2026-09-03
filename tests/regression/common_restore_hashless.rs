//! BUG 5 regression: unhashable files must not poison checkpoints.
//!
//! A file locked during hashing was recorded with `hash: null` and could
//! never be restored. New checkpoints omit it, retaining only the scan
//! warning. Verification remains strict for old malformed snapshots so it
//! cannot claim that an absent file was restored.

use crate::common::TestRepo;

#[test]
fn unhashable_file_is_omitted_with_warning_not_poisoned() {
    let repo = TestRepo::new();
    repo.write("ok.txt", b"normal content");

    // Make a file unhashable: unreadable permissions (Unix analog of
    // FileShare.None on Windows).
    #[cfg_attr(not(unix), allow(unused_variables))]
    let locked = repo.write("locked.txt", b"secret");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    }

    let scan = repo.scan_with_ignore();
    #[cfg_attr(not(unix), allow(unused_variables))]
    let locked_entry = scan
        .entries
        .iter()
        .find(|e| e.path.to_string_lossy().ends_with("locked.txt"));
    #[cfg(unix)]
    {
        assert!(
            locked_entry.is_none(),
            "unreadable file must not become an unrestorable snapshot entry"
        );
        assert!(
            scan.warnings
                .iter()
                .any(|w| w.path.ends_with("locked.txt") && w.message.contains("cannot hash file")),
            "unreadable file must be reported as a scan warning: {:?}",
            scan.warnings
        );
    }

    // The checkpoint must succeed (warning, not abort).
    let snapshot = repo.checkpoint_from_scan(&scan, "with locked");

    // And the stored snapshot must not contain the locked file or any
    // empty/null hash sentinel.
    assert!(
        snapshot
            .entries
            .iter()
            .all(|e| !e.path.to_string_lossy().ends_with("locked.txt")),
        "locked file must be absent from the snapshot"
    );
    for e in &snapshot.entries {
        if let Some(h) = &e.meta.hash {
            assert!(!h.is_empty(), "empty-string hash poisons snapshots");
        }
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}

#[test]
fn restore_skips_hashless_entries_with_warning() {
    let repo = TestRepo::new();
    repo.write("ok.txt", b"normal");

    // Hand-build a snapshot containing a hashless file entry (as if the
    // file was locked at checkpoint time).
    let ok_hash = varn::filesystem::hash_bytes(b"normal");
    repo.repo
        .object_store()
        .store_content(&ok_hash, b"normal")
        .unwrap();
    let mut entries = vec![crate::common::entry(
        "ok.txt",
        entry_kind_file(),
        Some(ok_hash.as_str()),
    )];
    // Match the real file's mtime so verification's metadata comparison
    // holds (the synthetic entry must describe the real file).
    entries[0].meta.mtime = crate::common::get_mtime(&repo.root().join("ok.txt"));
    let mut locked = crate::common::entry("locked.txt", entry_kind_file(), None);
    locked.meta.size = 6;
    entries.push(locked);

    // Store the objects the synthetic entries reference.
    for e in &entries {
        if let Some(h) = &e.meta.hash {
            let content = std::fs::read(repo.repo.root.join(&e.path)).unwrap_or_default();
            repo.repo.object_store().store_content(h, &content).unwrap();
        }
    }
    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: "hashless".to_string(),
        created_at: 1,
        root: repo.repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, entries);

    // Remove everything, then restore.
    std::fs::remove_file(repo.root().join("ok.txt")).unwrap();

    let plan = repo.plan_restore(&snapshot);
    let result = repo.execute(&plan);

    assert!(
        result
            .warnings
            .iter()
            .any(|w| w.contains("locked.txt") && w.contains("no content hash")),
        "expected a skip warning for the hashless file: {:?}",
        result.warnings
    );
    // The restorable file must still be restored.
    assert_eq!(repo.read_str("ok.txt"), "normal");
}

#[test]
fn verification_rejects_missing_hashless_entries_from_legacy_snapshots() {
    let repo = TestRepo::new();
    repo.write("ok.txt", b"normal");

    let ok_hash = varn::filesystem::hash_bytes(b"normal");
    repo.repo
        .object_store()
        .store_content(&ok_hash, b"normal")
        .unwrap();
    let mut entries = vec![crate::common::entry(
        "ok.txt",
        entry_kind_file(),
        Some(ok_hash.as_str()),
    )];
    entries[0].meta.mtime = crate::common::get_mtime(&repo.root().join("ok.txt"));
    entries.push(crate::common::entry("locked.txt", entry_kind_file(), None));
    // Store the objects the synthetic entries reference.
    for e in &entries {
        if let Some(h) = &e.meta.hash {
            let content = std::fs::read(repo.repo.root.join(&e.path)).unwrap_or_default();
            repo.repo.object_store().store_content(h, &content).unwrap();
        }
    }
    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: "hashless".to_string(),
        created_at: 1,
        root: repo.repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, entries);

    // Restore skips locked.txt (absent on disk). An old malformed snapshot
    // must now report verification failure instead of a false pass.
    repo.restore(&snapshot);
    assert!(
        !repo.verifies(&snapshot),
        "verification must count every legacy snapshot entry"
    );
}

#[test]
fn cache_never_reuses_hashless_entry_as_empty_string() {
    // The poisoning path: a cache entry with hash:None must not be served
    // back as Some("") on the next scan.
    let repo = TestRepo::new();
    #[cfg_attr(not(unix), allow(unused_variables))]
    let locked = repo.write("locked.txt", b"data");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    }

    let scan1 = repo.scan();
    // scan1 may have hash:None for locked.txt.

    // Second scan with the first scan's cache: the file is still
    // unhashable, the cache entry (if any) has hash None. The scanner must
    // not produce Some("").
    let mut scanner = varn::filesystem::Scanner::new(&repo.repo.root);
    scanner.set_cache(scan1.cache.clone());
    let scan2 = scanner.scan().unwrap();
    for e in &scan2.entries {
        if let Some(h) = &e.meta.hash {
            assert!(!h.is_empty(), "scanner produced an empty-string hash");
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
}

// Small helper so the import list stays identical across cfg branches.
fn entry_kind_file() -> varn::filesystem::EntryKind {
    varn::filesystem::EntryKind::File
}
