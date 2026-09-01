//! BUG 9 regression: ignore-rule semantics for diff/restore/verify.
//!
//! Field report (0.2.0): `.varnignore` itself was captured in checkpoints;
//! when the rules changed between checkpoint and restore, previously-ignored
//! files were flagged as extra, deleted, and verification failed. Fix: diff
//! and restore scan WITHOUT the current ignore rules — the checkpoint is a
//! self-contained state under its own rules.

use crate::common::TestRepo;
use std::fs;

#[test]
fn ignore_rules_exclude_files_from_checkpoint() {
    let repo = TestRepo::new();
    repo.write("keep.txt", b"keep");
    repo.write("debug.log", b"log");
    repo.write(".varnignore", b"*.log\n");

    let scan = repo.scan_with_ignore();
    let paths: Vec<String> = scan
        .entries
        .iter()
        .map(|e| e.path.to_string_lossy().replace('\\', "/"))
        .collect();
    assert!(
        paths.iter().all(|p| !p.ends_with(".log")),
        "ignored files must not be captured: {paths:?}"
    );
    assert!(paths.contains(&"keep.txt".to_string()));
}

#[test]
fn restore_uses_checkpoint_scope_not_current_rules() {
    let repo = TestRepo::new();
    repo.write("keep.txt", b"keep");
    repo.write("debug.log", b"log");
    fs::write(repo.root().join(".varnignore"), b"*.log\n").unwrap();

    // Checkpoint captured (with rules): keep.txt only.
    let snapshot = repo.checkpoint("with rules");

    // Agent state: keep.txt deleted; rules changed so *.log is no longer
    // ignored (debug.log now "exists" from the scanner's perspective).
    fs::remove_file(repo.root().join("keep.txt")).unwrap();
    fs::write(repo.root().join(".varnignore"), b"").unwrap();

    // Restore must bring back keep.txt and must NOT treat debug.log as an
    // unexpected file to delete (it was never part of the checkpoint's
    // world; deleting user data because rules changed would be wrong).
    let result = repo.restore(&snapshot);
    assert_eq!(repo.read_str("keep.txt"), "keep");
    assert!(
        repo.root().join("debug.log").exists(),
        "a file outside the checkpoint's scope must not be deleted"
    );
    let _ = result;
}

#[test]
fn diff_uses_checkpoint_scope_not_current_rules() {
    let repo = TestRepo::new();
    repo.write("keep.txt", b"keep");
    repo.write("debug.log", b"log");
    fs::write(repo.root().join(".varnignore"), b"*.log\n").unwrap();

    let snapshot = repo.checkpoint("with rules");

    // Rules loosened: debug.log is now visible to a rule-aware scan. The
    // diff against the checkpoint must NOT report it as ADDED.
    fs::write(repo.root().join(".varnignore"), b"").unwrap();
    let current = repo.scan(); // no ignore — checkpoint scope
    let changes = varn::diff::diff_states(&snapshot.entries, &current.entries);
    assert!(
        changes
            .iter()
            .all(|c| c.path != std::path::Path::new("debug.log")),
        "debug.log must not appear as a change: {changes:?}"
    );
}

#[test]
fn verification_passes_when_rules_changed() {
    let repo = TestRepo::new();
    repo.write("keep.txt", b"keep");
    repo.write("debug.log", b"log");
    fs::write(repo.root().join(".varnignore"), b"*.log\n").unwrap();

    let snapshot = repo.checkpoint("with rules");

    fs::remove_file(repo.root().join("keep.txt")).unwrap();
    fs::write(repo.root().join(".varnignore"), b"").unwrap();

    repo.restore(&snapshot);
    assert!(
        repo.verifies(&snapshot),
        "verification must pass after restore even though the current \
         .varnignore differs from the checkpoint's"
    );
}

#[test]
fn varnignore_itself_is_captured() {
    // The .varnignore file is part of the working state (it shapes future
    // checkpoints), so it IS captured — pinned behavior.
    let repo = TestRepo::new();
    repo.write("keep.txt", b"keep");
    repo.write(".varnignore", b"*.log\n");

    let snapshot = repo.checkpoint("rules captured");
    assert!(
        snapshot
            .entries
            .iter()
            .any(|e| e.path.to_string_lossy().replace('\\', "/") == ".varnignore")
    );
}
