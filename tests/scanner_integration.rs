//! Integration tests for the filesystem scanner.
//!
//! These exercise the Scanner against real directory trees created in
//! temporary directories, covering the filesystem correctness cases from
//! the spec: regular files, directories, symlinks, empty files, nested
//! directories, Unicode names, spaces, the `.varn/` skip rule, and
//! graceful error handling.

use std::fs;
use std::path::Path;
use tempfile::TempDir;
use varn::filesystem::{EntryKind, Scanner};
use varn::storage::VARN_DIR;

/// Helper: find an entry by relative path, panicking if not found.
fn find_entry<'a>(
    result: &'a varn::filesystem::ScanResult,
    name: &str,
) -> &'a varn::filesystem::TreeEntry {
    result
        .entries
        .iter()
        .find(|e| e.path == Path::new(name))
        .unwrap_or_else(|| panic!("entry not found: {name}"))
}

#[test]
fn scan_empty_directory() {
    let tmp = TempDir::new().unwrap();
    let result = Scanner::new(tmp.path()).scan().unwrap();
    assert!(result.entries.is_empty());
    assert!(result.warnings.is_empty());
}

#[test]
fn scan_single_file_with_correct_hash() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("hello.txt"), b"hello world").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    assert_eq!(result.entries.len(), 1);

    let entry = &result.entries[0];
    assert_eq!(entry.path, Path::new("hello.txt"));
    assert_eq!(entry.meta.kind, EntryKind::File);
    assert_eq!(entry.meta.size, 11);
    assert_eq!(
        entry.meta.hash.as_deref(),
        Some("b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9")
    );
}

#[test]
fn scan_nested_directories() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("src/utils")).unwrap();
    fs::write(tmp.path().join("src/main.rs"), b"fn main() {}").unwrap();
    fs::write(tmp.path().join("src/utils/helper.rs"), b"pub fn help() {}").unwrap();
    fs::write(tmp.path().join("README.md"), b"# Project").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();

    // 3 files + 2 directories (src, src/utils)
    assert_eq!(result.entries.len(), 5);

    let paths: Vec<String> = result
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();
    assert!(paths.contains(&"README.md".to_string()));
    assert!(paths.contains(&"src".to_string()));
    assert!(paths.contains(&"src/main.rs".to_string()));
    assert!(paths.contains(&"src/utils".to_string()));
    assert!(paths.contains(&"src/utils/helper.rs".to_string()));
}

#[test]
fn scan_entries_are_sorted_by_path() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("z.txt"), b"z").unwrap();
    fs::write(tmp.path().join("a.txt"), b"a").unwrap();
    fs::write(tmp.path().join("m.txt"), b"m").unwrap();
    fs::create_dir_all(tmp.path().join("b_dir")).unwrap();
    fs::write(tmp.path().join("b_dir/c.txt"), b"c").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let paths: Vec<String> = result
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();

    let mut expected = paths.clone();
    expected.sort();
    assert_eq!(paths, expected, "entries must be in sorted order");
}

#[test]
fn scan_identical_files_have_identical_hashes() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("a.txt"), b"same content").unwrap();
    fs::write(tmp.path().join("b.txt"), b"same content").unwrap();
    fs::write(tmp.path().join("c.txt"), b"different content").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let hash_a = find_entry(&result, "a.txt").meta.hash.as_deref().unwrap();
    let hash_b = find_entry(&result, "b.txt").meta.hash.as_deref().unwrap();
    let hash_c = find_entry(&result, "c.txt").meta.hash.as_deref().unwrap();

    assert_eq!(hash_a, hash_b);
    assert_ne!(hash_a, hash_c);
}

#[test]
fn scan_empty_file_has_empty_hash() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("empty.txt"), b"").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let entry = find_entry(&result, "empty.txt");
    assert_eq!(entry.meta.size, 0);
    assert_eq!(
        entry.meta.hash.as_deref(),
        Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
}

#[test]
fn scan_skips_varn_directory_at_root() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("real.txt"), b"data").unwrap();
    fs::create_dir_all(tmp.path().join(VARN_DIR).join("objects")).unwrap();
    fs::write(tmp.path().join(VARN_DIR).join("config.json"), b"{}").unwrap();
    fs::write(
        tmp.path().join(VARN_DIR).join("objects").join("abc123"),
        b"blob",
    )
    .unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let paths: Vec<String> = result
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();

    assert!(paths.iter().all(|p| !p.starts_with(".varn")));
    assert_eq!(paths, vec!["real.txt"]);
}

#[test]
fn scan_skips_varn_in_subdirectory() {
    // A .varn directory inside a subdirectory should also be skipped —
    // a nested .varn could be from a separately initialized sub-repo and
    // checkpointing/restoring it would corrupt that sub-repo's metadata.
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("project/.varn")).unwrap();
    fs::write(tmp.path().join("project/.varn/data"), b"not varn metadata").unwrap();
    fs::write(tmp.path().join("project/main.rs"), b"code").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let paths: Vec<String> = result
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().to_string())
        .collect();

    assert!(!paths.contains(&"project/.varn".to_string()));
    assert!(!paths.contains(&"project/.varn/data".to_string()));
    assert!(paths.contains(&"project/main.rs".to_string()));
}

#[test]
fn scan_records_symlink_as_symlink_not_followed() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("target.txt"), b"target content").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(tmp.path().join("target.txt"), tmp.path().join("link.txt"))
            .unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let link = find_entry(&result, "link.txt");
        assert_eq!(link.meta.kind, EntryKind::Symlink);
        assert_eq!(link.meta.hash, None);
    }
    #[cfg(not(unix))]
    {
        let _ = tmp;
    }
}

#[test]
fn scan_handles_unicode_filenames() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("café.txt"), b"coffee").unwrap();
    fs::write(tmp.path().join("日本語.txt"), b"japanese").unwrap();
    fs::write(tmp.path().join("emoji_😀.txt"), b"emoji").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    assert_eq!(result.entries.len(), 3);
    assert!(find_entry(&result, "café.txt").meta.kind == EntryKind::File);
    assert!(find_entry(&result, "日本語.txt").meta.kind == EntryKind::File);
    assert!(find_entry(&result, "emoji_😀.txt").meta.kind == EntryKind::File);
}

#[test]
fn scan_handles_spaces_and_special_chars_in_names() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("my file.txt"), b"spaced").unwrap();
    fs::write(tmp.path().join("file (1).txt"), b"parens").unwrap();
    fs::write(tmp.path().join("file[2].txt"), b"brackets").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    assert_eq!(result.entries.len(), 3);
}

#[test]
fn scan_records_directory_entries() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("subdir")).unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let dir = find_entry(&result, "subdir");
    assert_eq!(dir.meta.kind, EntryKind::Directory);
    assert_eq!(dir.meta.hash, None);
}

#[test]
fn scan_records_mtime() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("file.txt"), b"data").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let entry = find_entry(&result, "file.txt");
    assert!(entry.meta.mtime.is_some(), "mtime should be recorded");
    assert!(entry.meta.mtime.unwrap() > 0);
}

#[test]
fn scan_large_file_hashed_correctly() {
    let tmp = TempDir::new().unwrap();
    // 100KB of known content
    let content = vec![0xABu8; 100_000];
    fs::write(tmp.path().join("large.bin"), &content).unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    let entry = find_entry(&result, "large.bin");
    assert_eq!(entry.meta.size, 100_000);
    assert!(entry.meta.hash.is_some());

    // Verify the hash matches an independent computation.
    let expected = varn::filesystem::hash_bytes(&content);
    assert_eq!(entry.meta.hash.as_deref(), Some(expected.as_str()));
}

#[test]
fn scan_deeply_nested_tree() {
    let tmp = TempDir::new().unwrap();
    let mut path = tmp.path().to_path_buf();
    for i in 0..10 {
        path = path.join(format!("level{i}"));
        fs::create_dir_all(&path).unwrap();
    }
    fs::write(path.join("deep.txt"), b"deep").unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();
    // 10 directories + 1 file
    assert_eq!(result.entries.len(), 11);
    assert!(result.warnings.is_empty());
}

#[test]
fn scan_warning_for_unreadable_file() {
    let tmp = TempDir::new().unwrap();
    let f = tmp.path().join("locked.txt");
    fs::write(&f, b"secret").unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&f).unwrap().permissions();
        perms.set_mode(0o000);
        fs::set_permissions(&f, perms).unwrap();
    }

    let result = Scanner::new(tmp.path()).scan().unwrap();

    #[cfg(unix)]
    {
        // File should appear (metadata readable) but hash should be None.
        let entry = find_entry(&result, "locked.txt");
        assert_eq!(entry.meta.kind, EntryKind::File);
        assert!(entry.meta.hash.is_none());
        assert!(
            result
                .warnings
                .iter()
                .any(|w| { w.path == Path::new("locked.txt") && w.message.contains("hash") })
        );
    }
    #[cfg(not(unix))]
    {
        let _ = result;
    }
}

#[test]
fn scan_does_not_follow_symlink_to_directory() {
    let tmp = TempDir::new().unwrap();
    fs::create_dir_all(tmp.path().join("real_dir")).unwrap();
    fs::write(tmp.path().join("real_dir/inside.txt"), b"inside").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(tmp.path().join("real_dir"), tmp.path().join("link_dir"))
            .unwrap();

        let result = Scanner::new(tmp.path()).scan().unwrap();
        let link = find_entry(&result, "link_dir");
        assert_eq!(link.meta.kind, EntryKind::Symlink);
        // The contents of real_dir should NOT appear under link_dir.
        let paths: Vec<String> = result
            .entries
            .iter()
            .map(|e| e.path.to_string_lossy().to_string())
            .collect();
        assert!(
            !paths.iter().any(|p| p.starts_with("link_dir/")),
            "symlinked directory should not be traversed"
        );
    }
    #[cfg(not(unix))]
    {
        let _ = tmp;
    }
}

#[test]
fn scan_multiple_file_types() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("file1.txt"), b"text").unwrap();
    fs::write(tmp.path().join("file2.rs"), b"code").unwrap();
    fs::write(tmp.path().join("file3.json"), b"{}").unwrap();
    fs::create_dir_all(tmp.path().join("dir1")).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(tmp.path().join("file1.txt"), tmp.path().join("link1")).unwrap();

    let result = Scanner::new(tmp.path()).scan().unwrap();

    #[cfg(unix)]
    {
        assert_eq!(result.entries.len(), 5);
    }
    #[cfg(not(unix))]
    {
        assert_eq!(result.entries.len(), 4);
    }
}

#[test]
fn scan_result_is_deterministic() {
    let tmp = TempDir::new().unwrap();
    fs::write(tmp.path().join("b.txt"), b"b").unwrap();
    fs::write(tmp.path().join("a.txt"), b"a").unwrap();
    fs::create_dir_all(tmp.path().join("sub")).unwrap();
    fs::write(tmp.path().join("sub/c.txt"), b"c").unwrap();

    let result1 = Scanner::new(tmp.path()).scan().unwrap();
    let result2 = Scanner::new(tmp.path()).scan().unwrap();

    assert_eq!(result1.entries, result2.entries);
    assert_eq!(result1.warnings, result2.warnings);
}
