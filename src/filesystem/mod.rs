//! Filesystem data model and scanning.
//!
//! This module is split into:
//! - [`types`] — data models (`EntryKind`, `EntryMeta`, `TreeEntry`)
//! - [`scanner`] — the recursive directory `Scanner` and `hash_bytes` utility
//! - [`ignore`] — gitignore-style pattern matching for `.varnignore` files
//! - [`scan_cache`] — incremental scan cache for skipping unchanged files

pub mod ignore;
pub mod scan_cache;
pub mod scanner;
pub mod types;

// Re-export the public API so callers can use `varn::filesystem::Scanner`
// without knowing about the internal split.
pub use ignore::IgnoreRules;
pub use scan_cache::{CachedEntry, ScanCache};
pub use scanner::{ScanResult, ScanWarning, Scanner, hash_bytes, hash_file_path};
pub use types::{EntryKind, EntryMeta, TreeEntry};
