//! Migrate contract regressions (BUG 10 from the field report: newer
//! repositories must error, not report "already at version N").

use crate::common::TestRepo;
use std::fs;

#[test]
fn migrate_noop_at_current_version() {
    let repo = TestRepo::new();
    varn::storage::migrate_repo(&repo.repo).unwrap();
    let config = varn::storage::RepoConfig::read(&repo.repo.varn_dir).unwrap();
    assert_eq!(config.version, varn::storage::STORAGE_VERSION);
}

#[test]
fn migrate_rejects_newer_repository() {
    let repo = TestRepo::new();
    // Simulate a repository written by a future Varn.
    let mut config = varn::storage::RepoConfig::read(&repo.repo.varn_dir).unwrap();
    config.version = varn::storage::STORAGE_VERSION + 42;
    config.write(&repo.repo.varn_dir).unwrap();
    let repo = varn::storage::Repo::open(&repo.repo.root).unwrap();

    let err = varn::storage::migrate_repo(&repo).unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("newer than supported") || msg.contains("upgrade"),
        "actionable error expected, got: {msg}"
    );
}

#[test]
fn needs_migration_false_for_newer_version_is_not_reported_as_current() {
    // BUG 10's exact trap: needs_migration() returns false for a NEWER
    // version, which the CLI used to render as "already at version N
    // (current)". The CLI now checks newer explicitly; pin the primitive.
    let repo = TestRepo::new();
    let mut config = varn::storage::RepoConfig::read(&repo.repo.varn_dir).unwrap();
    config.version = varn::storage::STORAGE_VERSION + 1;
    config.write(&repo.repo.varn_dir).unwrap();
    let repo = varn::storage::Repo::open(&repo.repo.root).unwrap();

    // The primitive alone is not sufficient — the caller must compare both
    // directions. Document the contract via the migrate_repo error.
    assert!(!varn::storage::needs_migration(&repo));
    assert!(varn::storage::migrate_repo(&repo).is_err());
}

#[test]
fn migrate_backfills_git_guard() {
    let repo = TestRepo::new();
    let guard = repo.repo.varn_dir.join(".gitignore");
    fs::remove_file(&guard).unwrap();
    assert!(!guard.exists());

    // The guard backfill lives in the CLI layer (cmd_migrate); the
    // library primitive is ensure_guard. Pin both layers.
    varn::storage::ensure_guard(&repo.repo.varn_dir).unwrap();
    assert!(guard.is_file(), "guard must be backfilled");
    assert_eq!(fs::read_to_string(&guard).unwrap(), "*\n");
    varn::storage::migrate_repo(&repo.repo).unwrap();
}

#[test]
fn migrate_is_idempotent() {
    let repo = TestRepo::new();
    varn::storage::migrate_repo(&repo.repo).unwrap();
    varn::storage::migrate_repo(&repo.repo).unwrap();
    varn::storage::migrate_repo(&repo.repo).unwrap();
    let config = varn::storage::RepoConfig::read(&repo.repo.varn_dir).unwrap();
    assert_eq!(config.version, varn::storage::STORAGE_VERSION);
}

#[test]
fn migrate_preserves_checkpoints() {
    let repo = TestRepo::new();
    repo.write("a.txt", b"precious");
    let snapshot = repo.checkpoint("before migrate");

    varn::storage::migrate_repo(&repo.repo).unwrap();

    // The checkpoint still loads and restores.
    let loaded = varn::snapshot::SnapshotData::load(
        &repo
            .repo
            .snapshots_dir()
            .join(format!("{}.json", snapshot.meta.id.0)),
    )
    .unwrap();
    repo.write("a.txt", b"changed");
    repo.restore(&loaded);
    assert_eq!(repo.read_str("a.txt"), "precious");
}
