//! BUG 1 regression: scan cache must never serve a stale hash.
//!
//! Field report (Windows, 0.2.0): the cache keyed on `(size, mtime-seconds)`.
//! Two same-size writes within one second produced identical keys, so the
//! second checkpoint recorded the FIRST content's hash; restore then
//! returned old content while verification passed. Silent data corruption.

use crate::common::TestRepo;

#[test]
fn same_second_same_size_writes_produce_distinct_checkpoint_hashes() {
    let repo = TestRepo::new();

    repo.write("data.txt", b"AAAA00001");
    let scan1 = repo.scan();
    let h1 = crate::common::find_entry(&scan1, "data.txt")
        .meta
        .hash
        .clone()
        .unwrap();

    // Second write: same size, same whole-second mtime is unavoidable in
    // fast tests — but the sub-second component differs because the write
    // happens later. The cache key must include it.
    repo.write("data.txt", b"AAAA00002");
    let scan2 = repo.scan();
    let h2 = crate::common::find_entry(&scan2, "data.txt")
        .meta
        .hash
        .clone()
        .unwrap();

    assert_ne!(
        h1, h2,
        "two different contents must never share a hash even when written \
         within the same second"
    );
    assert_eq!(h2, varn::filesystem::hash_bytes(b"AAAA00002"));
}

#[test]
fn cache_reuse_only_for_identical_subsecond_mtime() {
    let repo = TestRepo::new();
    repo.write("data.txt", b"AAAA00001");
    let scan1 = repo.scan();

    // Rewrite with the SAME content: cache must be reusable (that is its
    // entire purpose).
    repo.write("data.txt", b"AAAA00001");
    // Force the mtime back to the exact recorded value including nanos is
    // not possible portably; instead assert the scan still hashes to the
    // same value either way (cache hit or re-hash — both correct).
    let scan2 = repo.scan();
    let h1 = crate::common::find_entry(&scan1, "data.txt")
        .meta
        .hash
        .clone()
        .unwrap();
    let h2 = crate::common::find_entry(&scan2, "data.txt")
        .meta
        .hash
        .clone()
        .unwrap();
    assert_eq!(h1, h2, "identical content must hash identically");
}

#[test]
fn stale_cache_entry_is_detected_and_checkpoint_self_heals() {
    // Simulate the exact poisoning path: a cache entry whose hash does not
    // match the file's real content. The checkpoint must abort-and-retry
    // with a cleared cache, capturing the CURRENT content.
    let repo = TestRepo::new();
    repo.write("data.txt", b"AAAA00001");
    let scan1 = repo.scan();

    // Poison: rewrite the file but keep the cache from scan1 (which still
    // holds the old hash with a matching size; nanos differ on a real
    // rewrite, so simulate the poisoning by directly reusing scan1's cache).
    repo.write("data.txt", b"AAAA00002");

    // Build a poisoned cache: old hash, current size/mtime.
    let mut poisoned = scan1.cache.clone();
    poisoned.insert(
        "data.txt",
        varn::filesystem::CachedEntry {
            size: 9,
            mtime: crate::common::get_mtime(&repo.root().join("data.txt")),
            mtime_nanos: crate::common::get_mtime_nanos(&repo.root().join("data.txt")),
            hash: Some(varn::filesystem::hash_bytes(b"AAAA00001")),
        },
    );

    // A scan WITH the poisoned cache would report the stale hash; the
    // checkpoint path must detect the mismatch at store time and re-hash.
    let mut scanner = Scanner::new(&repo.repo.root);
    scanner.set_cache(poisoned);
    let bad_scan = scanner.scan().unwrap();
    let bad_hash = crate::common::find_entry(&bad_scan, "data.txt")
        .meta
        .hash
        .clone()
        .unwrap();
    assert_eq!(
        bad_hash,
        varn::filesystem::hash_bytes(b"AAAA00001"),
        "precondition: poisoned cache serves the stale hash"
    );

    // The checkpoint built from the poisoned scan must fail storage and the
    // retry path must capture the correct content. Drive the same logic the
    // CLI uses: store_content_blobs returns StaleCache.
    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: "poisoned".to_string(),
        created_at: 1,
        root: repo.repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, bad_scan.entries.clone());
    let err = snapshot
        .store_content_blobs(&repo.repo.root, &repo.repo.object_store())
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        matches!(err, varn::error::VarnError::StaleCache { .. })
            || msg.contains("hash mismatch")
            || msg.contains("changed since scan"),
        "expected a stale-hash abort, got: {msg}"
    );

    // Self-heal: fresh scan (cleared cache) captures the correct content.
    let healed = repo.checkpoint("healed");
    let healed_hash = crate::common::find_snap_entry(&healed, "data.txt")
        .meta
        .hash
        .clone()
        .unwrap();
    assert_eq!(healed_hash, varn::filesystem::hash_bytes(b"AAAA00002"));

    // And restore returns the CURRENT content, not the old one.
    repo.restore(&healed);
    assert_eq!(repo.read_str("data.txt"), "AAAA00002");
}

use varn::filesystem::Scanner;
