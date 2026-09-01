//! Regression test suite for Varn.
//!
//! Organized by platform so every bug from every OS's field report has a
//! permanent, named test:
//!
//! - `common`  — bugs that reproduce on every platform (cache, IDs,
//!   verification, ignore semantics, git coexistence, storage)
//! - `windows` — Windows-specific behavior (attributes, ACLs, hard links,
//!   junctions, path forms). Compiled only on Windows targets.
//! - `macos`   — macOS-specific behavior (BSD flags). Compiled only on macOS.
//! - `linux`   — Linux-specific behavior (mode bits, uid/gid). Compiled only
//!   on Linux.
//!
//! Each test file is named after the bug or behavior it pins, and every
//! test carries a comment referencing the field report it came from.

// Shared helpers for all suites.
mod common;

// Cross-platform regressions (BUG 1-10 from the 0.2.0 Windows report, plus
// storage/diff/gc/ignore/git-coexistence contracts).
mod common_checkpoint_id;
mod common_cli_contracts;
mod common_diff_semantics;
mod common_empty_and_large_files;
mod common_gc_contracts;
mod common_git_coexistence;
mod common_hardlink_roundtrip;
mod common_ignore_semantics;
mod common_json_output;
mod common_long_paths;
mod common_migrate_contracts;
mod common_nested_directories;
mod common_restore_hashless;
mod common_restore_metadata;
mod common_restore_readonly;
mod common_restore_transactional;
mod common_safety_checkpoint;
mod common_scan_cache;
mod common_special_filenames;
mod common_storage_integrity;
mod common_symlink_roundtrip;
mod common_unicode_paths;

// Platform-specific suites. Cargo compiles `tests/regression/main.rs` once
// per target; the OS folders are gated so each platform runs its own.
#[cfg(windows)]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod linux;
