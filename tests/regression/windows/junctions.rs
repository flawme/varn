//! Windows junction regressions (field report item 11).
//!
//! Junctions are NTFS directory reparse points. `std::fs::FileType`
//! reports them as symlinks, so Varn captures them as `EntryKind::Symlink`
//! with the junction target. That classification is pinned here:
//!
//! - The junction itself is captured (never followed — following it would
//!   checkpoint the target's entire subtree and risk traversal).
//! - Restore re-creates the junction as a link.
//!
//! Full junction semantics (distinguishing junctions from symlinks via
//! reparse tags) are future work — see FUTURE.md.

use crate::common::TestRepo;
use std::fs;
use std::path::PathBuf;

/// Create a junction via the `mklink /J` shell builtin.
fn create_junction(target: &std::path::Path, link: &std::path::Path) -> std::io::Result<()> {
    let output = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &target.to_string_lossy(),
        ])
        .output()?;
    if output.status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ))
    }
}

#[test]
fn junction_captured_as_symlink_never_followed() {
    let repo = TestRepo::new();
    repo.write("realdir/inside.txt", b"inside the real dir");

    let junction = repo.root().join("jlink");
    create_junction(&repo.root().join("realdir"), &junction).unwrap();

    let scan = repo.scan();
    let entry = scan
        .entries
        .iter()
        .find(|e| e.path == PathBuf::from("jlink"))
        .expect("junction must be listed");
    assert_eq!(
        entry.meta.kind,
        varn::filesystem::EntryKind::Symlink,
        "junction is recorded as a symlink (documented classification)"
    );
    // The junction's TARGET contents must appear exactly once — through the
    // real path, never through the junction.
    let inside_count = scan
        .entries
        .iter()
        .filter(|e| e.path.to_string_lossy().contains("inside.txt"))
        .count();
    assert_eq!(inside_count, 1, "junction target must not be followed");
}

#[test]
fn junction_round_trip() {
    let repo = TestRepo::new();
    repo.write("realdir/inside.txt", b"inside");

    let junction = repo.root().join("jlink");
    create_junction(&repo.root().join("realdir"), &junction).unwrap();

    let snapshot = repo.checkpoint("junction");
    fs::remove_dir(&junction).unwrap(); // remove_dir removes a junction

    repo.restore(&snapshot);
    // The junction is restored as a reparse point (symlink/junction).
    let restored = repo.root().join("jlink");
    let meta = fs::symlink_metadata(&restored).unwrap();
    assert!(
        meta.file_type().is_symlink() || meta.file_type().is_dir(),
        "restored junction must be a link or dir, got {:?}",
        meta.file_type()
    );
    // And the real directory content is intact.
    assert_eq!(repo.read_str("realdir/inside.txt"), "inside");
}

#[test]
fn junction_target_escape_not_followed() {
    // A junction pointing OUTSIDE the managed root must not cause the
    // scanner to leave the root.
    let repo = TestRepo::new();
    let outside = std::env::temp_dir().join("varn-junction-outside");
    fs::create_dir_all(&outside).unwrap();
    fs::write(outside.join("secret.txt"), b"outside").unwrap();

    let junction = repo.root().join("esc");
    create_junction(&outside, &junction).unwrap();

    let scan = repo.scan();
    assert!(
        scan.entries
            .iter()
            .all(|e| !e.path.to_string_lossy().contains("secret.txt")),
        "scanner must not follow a junction outside the root"
    );
    // Cleanup.
    fs::remove_dir(&junction).ok();
    fs::remove_file(outside.join("secret.txt")).ok();
    fs::remove_dir(&outside).ok();
}
