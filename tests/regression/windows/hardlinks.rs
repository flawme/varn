//! Windows hard link regressions (BUG from 0.2.0: hard links were restored
//! as independent copies).

use crate::common::TestRepo;
use std::fs;

#[test]
fn hard_links_detected_on_ntfs() {
    let repo = TestRepo::new();
    repo.write("primary.txt", b"shared via link");
    fs::hard_link(
        repo.root().join("primary.txt"),
        repo.root().join("secondary.txt"),
    )
    .unwrap();

    let scan = repo.scan();
    let primary = crate::common::find_entry(&scan, "primary.txt");
    let secondary = crate::common::find_entry(&scan, "secondary.txt");
    assert!(
        primary.meta.nlink >= 2,
        "NTFS link count must be >= 2, got {}",
        primary.meta.nlink
    );
    assert_eq!(primary.meta.hardlink_to, None);
    assert_eq!(
        secondary.meta.hardlink_to.as_deref(),
        Some(std::path::Path::new("primary.txt"))
    );
}

#[test]
fn hard_link_round_trip_restores_link() {
    let repo = TestRepo::new();
    repo.write("primary.txt", b"shared via link");
    fs::hard_link(
        repo.root().join("primary.txt"),
        repo.root().join("secondary.txt"),
    )
    .unwrap();

    let snapshot = repo.checkpoint("hardlinks");
    fs::remove_file(repo.root().join("primary.txt")).unwrap();
    fs::remove_file(repo.root().join("secondary.txt")).unwrap();

    repo.restore(&snapshot);
    assert_eq!(repo.read_str("primary.txt"), "shared via link");
    assert_eq!(repo.read_str("secondary.txt"), "shared via link");

    // The relationship must be re-detected after restore.
    let scan = repo.scan();
    let secondary = crate::common::find_entry(&scan, "secondary.txt");
    assert_eq!(
        secondary.meta.hardlink_to.as_deref(),
        Some(std::path::Path::new("primary.txt")),
        "restored files must be hard-linked again, not independent copies"
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn readonly_hard_link_re_restore() {
    // BUG 3 interaction: a read-only hard-linked file must survive
    // repeated restores.
    let repo = TestRepo::new();
    let path = repo.write("primary.txt", b"readonly link");
    fs::hard_link(
        repo.root().join("primary.txt"),
        repo.root().join("secondary.txt"),
    )
    .unwrap();
    platform_shim::set_readonly(&path);

    let snapshot = repo.checkpoint("ro hardlink");
    fs::remove_file(repo.root().join("primary.txt")).unwrap();
    fs::remove_file(repo.root().join("secondary.txt")).unwrap();

    repo.restore(&snapshot);
    repo.restore(&snapshot); // the second restore overwrites read-only files
    assert_eq!(repo.read_str("primary.txt"), "readonly link");
    assert!(repo.verifies(&snapshot));
}

// Shim: set the readonly attribute (test-only helper).
mod platform_shim {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::SetFileAttributesW;

    pub fn set_readonly(path: &std::path::Path) {
        let wide: Vec<u16> = OsStr::new(path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            SetFileAttributesW(wide.as_ptr(), 0x1); // FILE_ATTRIBUTE_READONLY
        }
    }
}

use platform_shim::set_readonly as varn_platform_shim_set_readonly;
