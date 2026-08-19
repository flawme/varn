//! Integration tests for the `varn init` command and repository lifecycle.
//!
//! These tests exercise the CLI through the library API to verify the
//! end-to-end behavior of repository initialization, including the on-disk
//! layout and config persistence.

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;
use varn::platform;
use varn::storage::{Repo, RepoConfig, STORAGE_VERSION, VARN_DIR};

#[test]
fn init_creates_varn_directory_and_subdirs() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    let repo = Repo::init(root, "linux").unwrap();

    assert!(repo.varn_dir.is_dir());
    assert!(repo.varn_dir.join("objects").is_dir());
    assert!(repo.varn_dir.join("snapshots").is_dir());
    assert!(repo.varn_dir.join("index").is_dir());
    assert!(repo.varn_dir.join(RepoConfig::FILENAME).is_file());
}

#[test]
fn init_config_has_correct_version_and_platform() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();

    assert_eq!(repo.config.version, STORAGE_VERSION);
    assert_eq!(repo.config.platform, "linux");
    assert_eq!(repo.config.root, tmp.path());
    assert!(repo.config.created_at > 0);
}

#[test]
fn init_is_idempotent_failure() {
    let tmp = TempDir::new().unwrap();
    Repo::init(tmp.path(), "linux").unwrap();

    let err = Repo::init(tmp.path(), "linux").unwrap_err();
    assert!(matches!(
        err,
        varn::error::VarnError::AlreadyInitialized { .. }
    ));
}

#[test]
fn init_does_not_touch_user_files() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    // Create some user files first.
    fs::write(root.join("hello.txt"), b"hello").unwrap();
    fs::create_dir_all(root.join("subdir")).unwrap();
    fs::write(root.join("subdir/nested.txt"), b"nested").unwrap();

    Repo::init(root, "linux").unwrap();

    // User files must be untouched.
    assert_eq!(fs::read_to_string(root.join("hello.txt")).unwrap(), "hello");
    assert_eq!(
        fs::read_to_string(root.join("subdir/nested.txt")).unwrap(),
        "nested"
    );
}

#[test]
fn open_finds_repo_from_deep_subdirectory() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let created = Repo::init(root, "linux").unwrap();

    let deep = root.join("a/b/c/d");
    fs::create_dir_all(&deep).unwrap();

    let opened = Repo::open(&deep).unwrap();
    assert_eq!(opened.varn_dir, created.varn_dir);
    assert_eq!(opened.config, created.config);
}

#[test]
fn open_fails_outside_repo() {
    let tmp = TempDir::new().unwrap();
    let err = Repo::open(tmp.path()).unwrap_err();
    assert!(matches!(err, varn::error::VarnError::NotInitialized { .. }));
}

#[test]
fn exists_at_reflects_init_state() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();

    assert!(!Repo::exists_at(root));
    Repo::init(root, "linux").unwrap();
    assert!(Repo::exists_at(root));
}

#[test]
fn config_persists_across_reopen() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let created = Repo::init(root, "linux").unwrap();

    let reopened = Repo::open(root).unwrap();
    assert_eq!(reopened.config, created.config);
    assert_eq!(reopened.config.version, STORAGE_VERSION);
}

#[test]
fn varn_dir_constant_is_dot_varn() {
    assert_eq!(VARN_DIR, ".varn");
}

#[test]
fn init_with_different_platform_names() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "windows").unwrap();
    assert_eq!(repo.config.platform, "windows");
}

#[test]
fn platform_name_is_consistent() {
    let name = platform::os_name();
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), name).unwrap();
    assert_eq!(repo.config.platform, name);
}

#[test]
fn config_json_is_valid_and_pretty() {
    let tmp = TempDir::new().unwrap();
    let repo = Repo::init(tmp.path(), "linux").unwrap();
    let config_path = repo.varn_dir.join(RepoConfig::FILENAME);
    let raw = fs::read_to_string(&config_path).unwrap();

    // Pretty-printed JSON should contain newlines.
    assert!(raw.contains('\n'));

    // Must parse back to the same config.
    let parsed: RepoConfig = serde_json::from_str(&raw).unwrap();
    assert_eq!(parsed, repo.config);
}

#[test]
fn init_works_with_unicode_and_spaces_in_path() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("proyecto ñuñoa");
    fs::create_dir_all(&root).unwrap();

    let repo = Repo::init(&root, "linux").unwrap();
    assert_eq!(repo.root, root);
    assert!(repo.varn_dir.is_dir());
}

#[test]
fn init_works_with_relative_path_via_open() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    Repo::init(root, "linux").unwrap();

    // Change into the repo and open via ".".
    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(root).unwrap();
    let result = Repo::open(&PathBuf::from("."));
    std::env::set_current_dir(original).unwrap();

    assert!(result.is_ok());
}
