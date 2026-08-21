//! Restore engine: restoring a known state.
//!
//! The restore process follows a strict safety model:
//!
//! 1. **Plan** — compare the target snapshot with the current filesystem
//!    state and identify every action needed, plus any conflicts.
//! 2. **Confirm** — if conflicts exist, require explicit user confirmation
//!    (or the `--yes` flag).
//! 3. **Execute** — perform the actions: restore file contents from the
//!    object store, delete unexpected files, recreate directories.
//! 4. **Verify** — re-scan the filesystem and confirm it matches the
//!    snapshot.
//!
//! Restoration is treated as a potentially destructive operation. Files
//! that exist now but not in the checkpoint are deleted. Files that changed
//! since the checkpoint are overwritten. These actions are never performed
//! silently.
//!
//! This module is split into:
//! - [`plan`] — types, conflict detection, and `plan_restore`
//! - [`execute`] — `execute_restore` and filesystem safety checks
//! - [`verify`] — `verify_restore`

pub mod execute;
pub mod plan;
pub mod verify;

// Re-export the public API.
pub use execute::execute_restore;
pub use plan::{
    Conflict, RestoreAction, RestorePlan, RestoreResult, is_safe_relative_path, plan_restore,
};
pub use verify::verify_restore;
