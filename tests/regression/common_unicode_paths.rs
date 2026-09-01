//! Unicode filename regressions (field report: verified working — pinned).

use crate::common::TestRepo;

#[test]
fn unicode_filenames_round_trip() {
    let repo = TestRepo::new();
    let names = [
        "日本語.txt",   // Japanese
        "café.txt",     // Latin accents (NFC)
        "Ελληνικά.txt", // Greek
        "русский.txt",  // Cyrillic
        "emoji😀.txt",  // astral plane (surrogate pair on Windows)
    ];

    for (i, name) in names.iter().enumerate() {
        repo.write(name, format!("unicode {i}").as_bytes());
    }

    let snapshot = repo.checkpoint("unicode");
    for (i, name) in names.iter().enumerate() {
        assert_eq!(repo.read_str(name), format!("unicode {i}"));
    }

    for name in names.iter() {
        std::fs::remove_file(repo.root().join(name)).unwrap();
    }
    repo.restore(&snapshot);
    for (i, name) in names.iter().enumerate() {
        assert_eq!(repo.read_str(name), format!("unicode {i}"));
    }
    assert!(repo.verifies(&snapshot));
}

#[test]
fn unicode_directory_names_round_trip() {
    let repo = TestRepo::new();
    let rel = "目录/файл.txt";
    repo.write(rel, b"nested unicode");

    let snapshot = repo.checkpoint("unicode dirs");
    std::fs::remove_file(repo.root().join(rel)).unwrap();
    repo.restore(&snapshot);
    assert_eq!(repo.read_str(rel), "nested unicode");
    assert!(repo.verifies(&snapshot));
}

#[test]
fn unicode_names_in_snapshot_json_are_correct() {
    // The snapshot JSON must carry the exact bytes (the report's "cafAc"
    // console artifact was PowerShell rendering, not corruption).
    let repo = TestRepo::new();
    repo.write("café.txt", b"accented");

    let snapshot = repo.checkpoint("json check");
    let snap_path = repo
        .repo
        .snapshots_dir()
        .join(format!("{}.json", snapshot.meta.id.0));
    let raw = std::fs::read_to_string(&snap_path).unwrap();
    assert!(
        raw.contains("café.txt"),
        "snapshot JSON must contain the exact unicode filename"
    );
}

#[test]
#[cfg(not(target_os = "macos"))]
fn normalization_difference_is_a_different_file() {
    // NFC "café" vs NFD "cafe\u{301}" are different byte sequences and must
    // be treated as different files (no silent folding).
    //
    // SKIPPED on macOS: APFS is normalization-insensitive by default — the
    // filesystem itself folds NFC and NFD to the same name, so two files
    // that differ only by normalization cannot coexist there. The test
    // asserts Varn's byte-faithful behavior on normalization-sensitive
    // filesystems (ext4, NTFS).
    let repo = TestRepo::new();
    let nfc = "caf\u{e9}.txt";
    let nfd = "cafe\u{301}.txt";
    repo.write(nfc, b"composed");
    repo.write(nfd, b"decomposed");

    let snapshot = repo.checkpoint("normalization");
    assert_eq!(snapshot.entries.len(), 2); // 2 files (root not recorded) entry
    std::fs::remove_file(repo.root().join(nfc)).unwrap();
    std::fs::remove_file(repo.root().join(nfd)).unwrap();
    repo.restore(&snapshot);
    assert_eq!(repo.read_str(nfc), "composed");
    assert_eq!(repo.read_str(nfd), "decomposed");
}
