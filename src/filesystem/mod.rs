//! Filesystem data model and scanning.
//!
//! This module is split into:
//! - [`types`] — data models (`EntryKind`, `EntryMeta`, `TreeEntry`)
//! - [`scanner`] — the recursive directory `Scanner` and `hash_bytes` utility

pub mod scanner;
pub mod types;

// Re-export the public API so callers can use `varn::filesystem::Scanner`
// without knowing about the internal split.
pub use scanner::{ScanResult, ScanWarning, Scanner, hash_bytes};
pub use types::{EntryKind, EntryMeta, TreeEntry};
