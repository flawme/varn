//! Windows FileShare.None regressions.
//!
//! This uses the same exclusive sharing mode as the field report, rather
//! than approximating it with POSIX permissions. The scanner must report the
//! inaccessible file as a warning and keep it out of the snapshot.

use crate::common::TestRepo;
use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use windows_sys::Win32::Foundation::{CloseHandle, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, OPEN_EXISTING};

struct ExclusiveFile(HANDLE);

impl Drop for ExclusiveFile {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

fn open_with_no_sharing(path: &Path) -> ExclusiveFile {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            wide.as_ptr(),
            GENERIC_WRITE,
            0, // FileShare.None
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    assert_ne!(
        handle,
        INVALID_HANDLE_VALUE,
        "failed to open test file exclusively: {}",
        std::io::Error::last_os_error()
    );
    ExclusiveFile(handle)
}

#[test]
fn fileshare_none_file_is_warned_and_omitted_from_snapshot() {
    let repo = TestRepo::new();
    repo.write("ok.txt", b"restorable");
    let locked_path = repo.write("locked.txt", b"locked");
    let lock = open_with_no_sharing(&locked_path);

    let scan = repo.scan();
    assert!(
        scan.entries
            .iter()
            .all(|entry| entry.path != Path::new("locked.txt")),
        "a FileShare.None file must not become a hash:null snapshot entry"
    );
    assert!(
        scan.warnings.iter().any(|warning| {
            warning.path == Path::new("locked.txt") && warning.message.contains("cannot hash file")
        }),
        "the omitted file must be visible to callers: {:?}",
        scan.warnings
    );

    let snapshot = repo.checkpoint_from_scan(&scan, "with exclusive lock");
    assert!(
        snapshot
            .entries
            .iter()
            .all(|entry| entry.path != Path::new("locked.txt")),
        "checkpoint must not persist an unrestorable FileShare.None entry"
    );

    drop(lock);
    std::fs::remove_file(repo.root().join("ok.txt")).unwrap();
    std::fs::remove_file(&locked_path).unwrap();

    let result = repo.restore(&snapshot);
    assert!(
        result.files_written >= 1,
        "the restorable entry must be written: {result:?}"
    );
    assert_eq!(repo.read_str("ok.txt"), "restorable");
    assert!(
        !locked_path.exists(),
        "the skipped file must not be silently fabricated during restore"
    );
    assert!(
        repo.verifies(&snapshot),
        "verification must pass for every entry that was actually snapshotted"
    );
}
