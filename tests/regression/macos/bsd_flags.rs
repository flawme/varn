//! macOS BSD file flags (`st_flags`): capture via lstat, restore via
//! lchflags. Privileged flags are best-effort (warn, never fail).

use crate::common::TestRepo;
use std::fs;
use std::os::unix::ffi::OsStrExt;

unsafe extern "C" {
    fn lchflags(path: *const std::ffi::c_char, flags: u32) -> std::ffi::c_int;
}

fn get_flags(path: &std::path::Path) -> Option<u32> {
    // Reuse the scanner's capture path through a scan.
    let repo_root = path.ancestors().nth(1)?;
    let scan = varn::filesystem::Scanner::new(repo_root).scan().ok()?;
    let rel = path.file_name()?.to_str()?;
    scan.entries
        .iter()
        .find(|e| e.path.file_name()?.to_str() == Some(rel))
        .and_then(|e| e.meta.flags)
}

fn set_flags_raw(path: &std::path::Path, flags: u32) -> bool {
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
    unsafe { lchflags(c.as_ptr(), flags) == 0 }
}

#[test]
fn hidden_flag_captured_and_restored() {
    // UF_HIDDEN = 0x8000 — unprivileged, settable by the owner.
    const UF_HIDDEN: u32 = 0x8000;

    let repo = TestRepo::new();
    let path = repo.write("hidden.txt", b"hidden content");
    assert!(set_flags_raw(&path, UF_HIDDEN), "lchflags must succeed");

    let snapshot = repo.checkpoint("hidden");
    let entry = crate::common::find_snap_entry(&snapshot, "hidden.txt");
    assert_eq!(entry.meta.flags, Some(UF_HIDDEN), "flags must be captured");

    // Clear the flag, then restore: it must come back.
    set_flags_raw(&path, 0);
    repo.restore(&snapshot);
    assert_eq!(
        get_flags(&path),
        Some(UF_HIDDEN),
        "hidden flag must be restored"
    );
    assert!(repo.verifies(&snapshot));
}

#[test]
fn uchg_immutable_flag_best_effort() {
    // UF_IMMUTABLE = 0x2 — settable by owner, but an immutable file cannot
    // be modified or deleted until the flag is cleared. Restore must clear
    // protection (the flag) before overwriting.
    const UF_IMMUTABLE: u32 = 0x2;

    let repo = TestRepo::new();
    let path = repo.write("imm.txt", b"immutable content");
    assert!(set_flags_raw(&path, UF_IMMUTABLE));

    let snapshot = repo.checkpoint("immutable");

    // Clear, drift, restore: the flag must be re-applied and the restore
    // must not fail.
    set_flags_raw(&path, 0);
    fs::write(&path, b"drifted").unwrap();
    repo.restore(&snapshot);
    assert_eq!(repo.read_str("imm.txt"), "immutable content");
    assert_eq!(get_flags(&path), Some(UF_IMMUTABLE));

    // Cleanup: clear the flag so the temp dir can be removed.
    set_flags_raw(&path, 0);
}

#[test]
fn flags_of_zero_are_stored_as_none_or_zero_consistently() {
    let repo = TestRepo::new();
    repo.write("plain.txt", b"plain");

    let snapshot = repo.checkpoint("plain");
    let entry = crate::common::find_snap_entry(&snapshot, "plain.txt");
    // A normal file has flags 0; the scanner records Some(0) on macOS.
    assert!(
        entry.meta.flags == Some(0) || entry.meta.flags.is_none(),
        "unexpected flags value: {:?}",
        entry.meta.flags
    );
}

#[test]
fn flags_drift_detected_by_diff() {
    const UF_HIDDEN: u32 = 0x8000;
    let repo = TestRepo::new();
    let path = repo.write("f.txt", b"data");
    let snapshot = repo.checkpoint("base");

    set_flags_raw(&path, UF_HIDDEN);
    let current = repo.scan();
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .any(|c| c.path == std::path::Path::new("f.txt")),
        "flags-only drift must appear in diff"
    );

    set_flags_raw(&path, 0);
    repo.restore(&snapshot);
    set_flags_raw(&path, 0); // cleanup
}
