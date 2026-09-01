//! Windows ACL / security descriptor regressions (BUG 7: NUL-padded SDDL
//! made every restore fail with os error 87).

use crate::common::TestRepo;
use std::fs;
use std::os::windows::fs::MetadataExt;

#[test]
fn acl_captured_without_nul_padding() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"with acl");

    let snapshot = repo.checkpoint("acl");
    let entry = crate::common::find_snap_entry(&snapshot, "a.txt");
    if let Some(sddl) = &entry.meta.acl {
        assert!(
            !sddl.contains('\0'),
            "SDDL string must not contain NUL padding (BUG 7): {sddl:?}"
        );
        assert!(
            sddl.starts_with("O:") || sddl.starts_with("G:") || sddl.starts_with("D:"),
            "SDDL string must look like SDDL: {sddl:?}"
        );
    }
    // If acl is None (capture refused), the test still passes — capture is
    // advisory. The NUL-padding bug only manifests when Some.
}

#[test]
fn acl_restore_succeeds_or_warns_never_error87_spam() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"with acl");
    let snapshot = repo.checkpoint("acl");

    repo.write("a.txt", b"changed");
    let result = repo.restore(&snapshot);

    // If ACL restore is attempted, it must not fail with os error 87
    // (ERROR_INVALID_PARAMETER — the NUL-padding symptom).
    for w in &result.warnings {
        assert!(
            !w.contains("os error 87"),
            "ACL restore must not fail with error 87 (NUL padding): {w}"
        );
    }
    assert!(repo.verifies(&snapshot));
}

#[test]
fn legacy_padded_acl_snapshot_still_restores() {
    // A snapshot written by 0.2.0 may contain NUL-padded SDDL. Restore
    // must strip the padding and proceed (warn at most), never fail.
    let repo = TestRepo::new();
    repo.write("a.txt", b"legacy acl");

    let mut entries: Vec<_> = repo.scan().entries;
    for e in entries.iter_mut() {
        if e.path == std::path::Path::new("a.txt") {
            e.meta.acl = Some("O:S-1-5-21G:DUD:(A;OICIID;FA;;;AU)\0\0\0\0".to_string());
        }
    }
    // Store the content object for the entry.
    let hash = entries
        .iter()
        .find(|e| e.path == std::path::Path::new("a.txt"))
        .unwrap()
        .meta
        .hash
        .clone()
        .unwrap();
    repo.repo
        .object_store()
        .store_content(&hash, b"legacy acl")
        .unwrap();

    let meta = varn::core::CheckpointMeta {
        id: varn::core::CheckpointId("pending".to_string()),
        description: "legacy padded acl".to_string(),
        created_at: 1,
        root: repo.repo.root.clone(),
    };
    let snapshot = varn::snapshot::SnapshotData::new(meta, entries);

    fs::remove_file(repo.root().join("a.txt")).unwrap();
    let result = repo.restore(&snapshot);
    for w in &result.warnings {
        assert!(
            !w.contains("os error 87"),
            "padded legacy SDDL must be cleaned before use: {w}"
        );
    }
    assert_eq!(repo.read_str("a.txt"), "legacy acl");
}

#[test]
fn attributes_and_acl_together() {
    // The full Windows metadata set on one file.
    let repo = TestRepo::new();
    let path = repo.write("full.txt", b"full metadata");
    // Set readonly via attributes (archive default + readonly).
    platform_shim::set_attrs(&path, 0x21); // READONLY | ARCHIVE

    let snapshot = repo.checkpoint("full");
    let entry = crate::common::find_snap_entry(&snapshot, "full.txt");
    assert_eq!(entry.meta.attributes, Some(0x21));

    // Clear and restore.
    platform_shim::set_attrs(&path, 0x20);
    repo.restore(&snapshot);
    let now = fs::symlink_metadata(&path).unwrap().file_attributes();
    assert_eq!(now & 0x1, 0x1, "readonly must be restored");
    assert!(repo.verifies(&snapshot));
    platform_shim::set_attrs(&path, 0x20); // cleanup
}

// Shim for direct attribute manipulation in tests.
mod platform_shim {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

    pub fn set_attrs(path: &std::path::Path, attributes: u32) {
        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            SetFileAttributesW(wide.as_ptr(), attributes);
        }
    }
}
