//! Varn — local state checkpointing and rollback.
//!
//! This is the library root. The binary entry point is in `main.rs`.

pub mod cli;
pub mod core;
pub mod diff;
pub mod error;
pub mod filesystem;
pub mod platform;
pub mod restore;
pub mod snapshot;
pub mod storage;
