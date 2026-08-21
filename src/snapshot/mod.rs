//! Snapshot engine: creating checkpoints and representing state.
//!
//! This module is split into:
//! - [`data`] — `SnapshotData` struct, persistence, content blob storage
//! - [`id`] — checkpoint ID generation and validation

pub mod data;
pub mod id;

// Re-export the public API.
pub use data::{Snapshot, SnapshotData};
