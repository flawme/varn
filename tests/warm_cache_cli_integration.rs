//! End-to-end warm-cache checkpoint coverage.
//!
//! After the initial checkpoint, source reads are no longer needed when the
//! scan-cache metadata matches and the content object is already present.
//! Removing read permission makes an accidental second hash read observable.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use tempfile::TempDir;
use varn::storage::Repo;

#[test]
fn warm_cli_checkpoint_reuses_existing_object_without_reopening_source() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("project");
    fs::create_dir_all(&root).unwrap();
    Repo::init(&root, varn::platform::os_name()).unwrap();

    let source = root.join("large-enough-to-notice.txt");
    fs::write(&source, b"cached content").unwrap();

    let first = Command::new(env!("CARGO_BIN_EXE_varn"))
        .args(["--json", "checkpoint", "initial"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        first.status.success(),
        "initial checkpoint failed: {}",
        String::from_utf8_lossy(&first.stderr)
    );

    // Metadata remains available, but opening the file for a second hash
    // would fail. Restore permissions before TempDir cleanup even on panic.
    fs::set_permissions(&source, fs::Permissions::from_mode(0o000)).unwrap();
    let second = Command::new(env!("CARGO_BIN_EXE_varn"))
        .args(["--json", "checkpoint", "warm"])
        .current_dir(&root)
        .output()
        .unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o644)).unwrap();

    assert!(
        second.status.success(),
        "warm checkpoint unexpectedly reopened the cached file: {}",
        String::from_utf8_lossy(&second.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(json["entries"], 1);
    assert!(
        json["warnings"].as_array().unwrap().is_empty(),
        "warm cache hit must not emit a read failure: {json}"
    );
}
