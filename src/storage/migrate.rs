//! Storage format migration.
//!
//! When the on-disk format changes in a way that requires migration, a new
//! migration step is added here. Each step upgrades from version N to N+1.
//!
//! The migration framework:
//! 1. Reads the current storage version from `config.json`.
//! 2. If it's older than [`STORAGE_VERSION`], applies migrations sequentially.
//! 3. Updates the version in `config.json` after each successful step.
//!
//! Migrations are idempotent and safe to re-run: if a migration is
//! interrupted, re-running it will detect the current version and resume
//! from where it left off.
//!
//! Currently there is only version 1, so no migrations exist yet. The
//! framework is in place for future format changes.

use crate::error::{Result, VarnError};
use crate::storage::repo::{Repo, RepoConfig, STORAGE_VERSION};
use std::path::Path;

/// A migration step that upgrades the storage format from one version to
/// the next.
struct Migration {
    /// The version this migration upgrades FROM.
    from_version: u32,
    /// The version this migration upgrades TO.
    to_version: u32,
    /// A description of what this migration does.
    #[allow(dead_code)]
    description: &'static str,
    /// The migration function.
    migrate: fn(&Repo) -> Result<()>,
}

/// All registered migrations, ordered by `from_version`.
///
/// To add a migration from version 1 to 2, add a new `Migration` entry here.
fn migrations() -> Vec<Migration> {
    vec![
        // Example for future use:
        // Migration {
        //     from_version: 1,
        //     to_version: 2,
        //     description: "Add hard link inode index",
        //     migrate: migrate_v1_to_v2,
        // },
    ]
}

/// Check if the repository's storage format needs migration.
///
/// Returns `true` if the current version is less than [`STORAGE_VERSION`].
pub fn needs_migration(repo: &Repo) -> bool {
    repo.config.version < STORAGE_VERSION
}

/// Run all pending migrations on a repository.
///
/// This reads the current version from `config.json`, applies migrations
/// sequentially, and updates the version after each step.
///
/// If the repository is already at the current version, this is a no-op.
/// If the repository is at a NEWER version than the code expects, an error
/// is returned (the code cannot downgrade).
pub fn migrate_repo(repo: &Repo) -> Result<()> {
    let mut current_version = repo.config.version;

    if current_version > STORAGE_VERSION {
        return Err(VarnError::Other(format!(
            "repository version {} is newer than supported version {}; \
             please upgrade Varn",
            current_version, STORAGE_VERSION
        )));
    }

    let steps = migrations();

    for step in &steps {
        if current_version == step.from_version {
            (step.migrate)(repo)?;
            current_version = step.to_version;
            // Update the version in config.json.
            update_version(&repo.varn_dir, current_version)?;
        }
    }

    if current_version < STORAGE_VERSION {
        return Err(VarnError::Other(format!(
            "no migration path from version {} to {}; \
             repository may have been created by an incompatible Varn version",
            current_version, STORAGE_VERSION
        )));
    }

    Ok(())
}

/// Update the version field in `config.json`.
fn update_version(varn_dir: &Path, version: u32) -> Result<()> {
    let mut config = RepoConfig::read(varn_dir)?;
    config.version = version;
    config.write(varn_dir)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::Repo;
    use tempfile::TempDir;

    #[test]
    fn needs_migration_false_for_current_version() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        assert!(!needs_migration(&repo));
    }

    #[test]
    fn needs_migration_true_for_older_version() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        // Manually set an older version.
        let mut config = RepoConfig::read(&repo.varn_dir).unwrap();
        config.version = 0;
        config.write(&repo.varn_dir).unwrap();
        let repo = Repo::open(tmp.path()).unwrap();
        assert!(needs_migration(&repo));
    }

    #[test]
    fn migrate_repo_noop_for_current_version() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        migrate_repo(&repo).unwrap();
        // Version should still be current.
        let config = RepoConfig::read(&repo.varn_dir).unwrap();
        assert_eq!(config.version, STORAGE_VERSION);
    }

    #[test]
    fn migrate_repo_rejects_newer_version() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        // Manually set a newer version.
        let mut config = RepoConfig::read(&repo.varn_dir).unwrap();
        config.version = STORAGE_VERSION + 1;
        config.write(&repo.varn_dir).unwrap();
        let repo = Repo::open(tmp.path()).unwrap();
        let err = migrate_repo(&repo).unwrap_err();
        assert!(err.to_string().contains("newer than supported"));
    }

    #[test]
    fn migrate_repo_no_migrations_registered() {
        // Currently no migrations are registered, so a repo at version 0
        // cannot be migrated.
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        let mut config = RepoConfig::read(&repo.varn_dir).unwrap();
        config.version = 0;
        config.write(&repo.varn_dir).unwrap();
        let repo = Repo::open(tmp.path()).unwrap();
        let err = migrate_repo(&repo).unwrap_err();
        assert!(err.to_string().contains("no migration path"));
    }

    #[test]
    fn update_version_persists() {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), "linux").unwrap();
        update_version(&repo.varn_dir, 42).unwrap();
        let config = RepoConfig::read(&repo.varn_dir).unwrap();
        assert_eq!(config.version, 42);
    }
}
