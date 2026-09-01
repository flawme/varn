//! macOS-specific regression suite.
//!
//! Compiled and run only on macOS targets. Covers BSD file flags
//! (capture + best-effort restore) and macOS symlink semantics.

mod bsd_flags;
mod macos_symlinks;
