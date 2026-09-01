//! Empty-file and large-file regressions (field report: 1 GB file hashed
//! at ~300 MB/s cold, 45 ms warm — performance pinned loosely, correctness
//! pinned exactly).

use crate::common::TestRepo;
use std::fs;

#[test]
fn empty_file_round_trip() {
    let repo = TestRepo::new();
    repo.write("empty.txt", b"");

    let snapshot = repo.checkpoint("empty");
    let entry = crate::common::find_snap_entry(&snapshot, "empty.txt");
    assert_eq!(entry.meta.size, 0);
    // SHA-256 of empty input.
    assert_eq!(
        entry.meta.hash.as_deref(),
        Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );

    fs::remove_file(repo.root().join("empty.txt")).unwrap();
    repo.restore(&snapshot);
    assert!(repo.root().join("empty.txt").is_file());
    assert_eq!(repo.read("empty.txt").len(), 0);
    assert!(repo.verifies(&snapshot));
}

#[test]
fn empty_vs_missing_are_different() {
    let repo = TestRepo::new();
    repo.write("empty.txt", b"");
    let snapshot = repo.checkpoint("empty");

    fs::remove_file(repo.root().join("empty.txt")).unwrap();
    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("empty.txt"))
    );

    repo.restore(&snapshot);
    assert!(repo.root().join("empty.txt").is_file());
}

#[test]
fn multi_mb_file_round_trip() {
    // A few MB: exercises the streaming/64KB-chunk path without the cost
    // of the report's 1 GB case (CI-friendly).
    let repo = TestRepo::new();
    let big: Vec<u8> = (0..5u32)
        .flat_map(|i| (0..1_000_000u32).map(move |j| (i * 31 + j * 7) as u8))
        .collect();
    repo.write("big.bin", &big);

    let snapshot = repo.checkpoint("big");
    let entry = crate::common::find_snap_entry(&snapshot, "big.bin");
    assert_eq!(entry.meta.size, big.len() as u64);

    fs::remove_file(repo.root().join("big.bin")).unwrap();
    repo.restore(&snapshot);
    assert_eq!(
        repo.read("big.bin"),
        big,
        "large file must round-trip exactly"
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn large_file_partial_change_detected() {
    let repo = TestRepo::new();
    let mut big = vec![0u8; 3_000_000];
    for (i, b) in big.iter_mut().enumerate() {
        *b = (i % 251) as u8;
    }
    repo.write("big.bin", &big);
    let snapshot = repo.checkpoint("big v1");

    // Change ONE byte in the middle.
    big[1_500_000] ^= 0xff;
    repo.write("big.bin", &big);

    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("big.bin")),
        "a single-byte change in a large file must be detected"
    );

    repo.restore(&snapshot);
    assert_eq!(repo.read("big.bin"), {
        let mut orig = vec![0u8; 3_000_000];
        for (i, b) in orig.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        orig
    });
    assert!(repo.verifies(&snapshot));
}

#[test]
fn binary_content_with_all_byte_values_round_trips() {
    let repo = TestRepo::new();
    let all_bytes: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
    repo.write("binary.bin", &all_bytes);

    let snapshot = repo.checkpoint("binary");
    fs::remove_file(repo.root().join("binary.bin")).unwrap();
    repo.restore(&snapshot);
    assert_eq!(repo.read("binary.bin"), all_bytes);
    assert!(repo.verifies(&snapshot));
}
