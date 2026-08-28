//! Storage layer: on-disk layout and persistence for Varn.
//!
//! The repository layout is:
//!
//! ```text
//! .varn/
//! ├── config.json       (repository configuration)
//! ├── objects/          (content-addressed blobs)
//! ├── snapshots/        (snapshot metadata as JSON)
//! └── index/            (fast lookups, future)
//! ```
//!
//! This module is split into:
//! - [`repo`] — `Repo`, `RepoConfig`, repository discovery
//! - [`object_store`] — `ObjectStore` for content-addressed blobs
//! - [`gc`] — garbage collection
//! - [`migrate`] — storage format migration

pub mod gc;
pub mod migrate;
pub mod object_store;
pub mod repo;

// Re-export the public API.
pub use gc::{GcResult, garbage_collect};
pub use migrate::{migrate_repo, needs_migration};
pub use object_store::ObjectStore;
pub use repo::{Repo, RepoConfig, STORAGE_VERSION, VARN_DIR, find_varn_dir};
