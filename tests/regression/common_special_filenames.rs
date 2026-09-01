//! Special-filename regressions (field report item 14: these all worked —
//! pinned so they keep working).

use crate::common::TestRepo;

#[test]
fn filenames_with_spaces_and_special_chars_round_trip() {
    let repo = TestRepo::new();
    let names = [
        "file with spaces.txt",
        "exclamation!.txt",
        "hash#.txt",
        "amp&ersand.txt",
        "bracket[1].txt",
        "-dash-start.txt",
        "dot.in.name.txt",
        "multiple   spaces.txt",
        "trailing.dot.",
    ];

    for (i, name) in names.iter().enumerate() {
        repo.write(name, format!("content {i}").as_bytes());
    }

    let snapshot = repo.checkpoint("special names");
    for (i, name) in names.iter().enumerate() {
        assert_eq!(
            repo.read_str(name),
            format!("content {i}"),
            "content mismatch for {name}"
        );
    }

    // Wreck and restore.
    for name in names.iter() {
        fs_extra_delete(&repo.root().join(name));
    }
    repo.restore(&snapshot);
    for (i, name) in names.iter().enumerate() {
        assert_eq!(repo.read_str(name), format!("content {i}"));
    }
    assert!(repo.verifies(&snapshot));
}

#[test]
fn reserved_and_tricky_names_handled() {
    let repo = TestRepo::new();
    // Names that are tricky on one platform or another.
    let names = ["a.b.c.txt", "..dots..txt", "~tilde.txt", "%percent.txt"];
    for (i, name) in names.iter().enumerate() {
        repo.write(name, format!("v{i}").as_bytes());
    }
    let snapshot = repo.checkpoint("tricky");
    repo.restore(&snapshot);
    for (i, name) in names.iter().enumerate() {
        assert_eq!(repo.read_str(name), format!("v{i}"));
    }
    assert!(repo.verifies(&snapshot));
}

#[test]
fn deep_path_with_special_components() {
    let repo = TestRepo::new();
    let rel = "dir with space/sub[1]/#hash/file name.txt";
    repo.write(rel, b"deep special");
    let snapshot = repo.checkpoint("deep special");

    fs_extra_delete(&repo.root().join(rel));
    repo.restore(&snapshot);
    assert_eq!(repo.read_str(rel), "deep special");
    assert!(repo.verifies(&snapshot));
}

fn fs_extra_delete(path: &std::path::Path) {
    if path.is_dir() {
        std::fs::remove_dir_all(path).unwrap();
    } else if path.exists() {
        std::fs::remove_file(path).unwrap();
    }
}
