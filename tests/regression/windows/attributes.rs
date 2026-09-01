//! Windows file attribute regressions (READONLY, HIDDEN, SYSTEM, ARCHIVE).

use crate::common::TestRepo;
use std::fs;
use std::os::windows::fs::MetadataExt;

const FILE_ATTRIBUTE_READONLY: u32 = 0x1;
const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
const FILE_ATTRIBUTE_ARCHIVE: u32 = 0x20;

fn attrs_of(path: &std::path::Path) -> u32 {
    fs::symlink_metadata(path).unwrap().file_attributes()
}

fn set_attrs(path: &std::path::Path, attrs: u32) {
    varn::platform_set_file_attributes(path, attrs);
}

// Tiny shim so tests don't reach into cfg-gated platform internals
// inconsistently; the public path is via the restore engine, but tests
// need direct manipulation too.
mod platform_shim {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

    pub fn set_file_attributes(path: &std::path::Path, attributes: u32) {
        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            SetFileAttributesW(wide.as_ptr(), attributes);
        }
    }
}

use platform_shim::set_file_attributes as platform_set_file_attributes;

#[test]
fn readonly_attribute_captured_and_restored() {
    let repo = TestRepo::new();
    let path = repo.write("ro.txt", b"readonly");
    set_attrs(&path, FILE_ATTRIBUTE_READONLY);

    let snapshot = repo.checkpoint("ro");
    let entry = crate::common::find_snap_entry(&snapshot, "ro.txt");
    assert_eq!(
        entry.meta.attributes,
        Some(FILE_ATTRIBUTE_READONLY),
        "readonly attribute must be captured"
    );

    // Clear, drift, restore: attribute must come back.
    set_attrs(&path, FILE_ATTRIBUTE_ARCHIVE);
    repo.restore(&snapshot);
    assert_eq!(
        attrs_of(&path) & FILE_ATTRIBUTE_READONLY,
        FILE_ATTRIBUTE_READONLY
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn hidden_and_system_captured_and_restored() {
    let repo = TestRepo::new();
    let path = repo.write("sys.txt", b"system file");
    set_attrs(
        &path,
        FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM | FILE_ATTRIBUTE_ARCHIVE,
    );

    let snapshot = repo.checkpoint("sys");
    let entry = crate::common::find_snap_entry(&snapshot, "sys.txt");
    assert_eq!(
        entry.meta.attributes,
        Some(FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM | FILE_ATTRIBUTE_ARCHIVE)
    );

    set_attrs(&path, FILE_ATTRIBUTE_ARCHIVE);
    repo.restore(&snapshot);
    let now = attrs_of(&path);
    assert_eq!(now & FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_HIDDEN);
    assert_eq!(now & FILE_ATTRIBUTE_SYSTEM, FILE_ATTRIBUTE_SYSTEM);
    assert!(repo.verifies(&snapshot));
    // Cleanup so temp dir removal works.
    set_attrs(&path, FILE_ATTRIBUTE_ARCHIVE);
}

#[test]
fn archive_attribute_round_trip() {
    let repo = TestRepo::new();
    let path = repo.write("arch.txt", b"archive");
    // A fresh file has ARCHIVE set by default; pin it explicitly.
    assert_ne!(attrs_of(&path) & FILE_ATTRIBUTE_ARCHIVE, 0);

    let snapshot = repo.checkpoint("arch");
    set_attrs(&path, 0); // clear everything
    repo.restore(&snapshot);
    assert_ne!(
        attrs_of(&path) & FILE_ATTRIBUTE_ARCHIVE,
        0,
        "archive attribute must be restored"
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn attribute_drift_detected_by_diff() {
    let repo = TestRepo::new();
    let path = repo.write("f.txt", b"data");
    let snapshot = repo.checkpoint("base");

    set_attrs(&path, FILE_ATTRIBUTE_READONLY);
    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("f.txt")),
        "attribute-only drift must appear in diff"
    );

    set_attrs(&path, FILE_ATTRIBUTE_ARCHIVE);
    repo.restore(&snapshot);
}

#[test]
fn normal_file_has_no_spurious_attributes() {
    let repo = TestRepo::new();
    repo.write("normal.txt", b"normal");
    let snapshot = repo.checkpoint("normal");
    let entry = crate::common::find_snap_entry(&snapshot, "normal.txt");
    let attrs = entry.meta.attributes.unwrap();
    // ARCHIVE is set on new files; READONLY/HIDDEN/SYSTEM must be clear.
    assert_eq!(attrs & FILE_ATTRIBUTE_READONLY, 0);
    assert_eq!(attrs & FILE_ATTRIBUTE_HIDDEN, 0);
    assert_eq!(attrs & FILE_ATTRIBUTE_SYSTEM, 0);
}
