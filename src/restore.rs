//! Restore engine: restoring a known state.
//!
//! This module is a placeholder for the full restore engine. The safety
//! model (conflict detection, explicit confirmation, verification) will be
//! implemented here.

/// A conflict detected during restore planning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// A file was modified since the checkpoint and would be overwritten.
    Modified { path: String },
    /// A file exists now but not in the checkpoint, and would be deleted.
    Unexpected { path: String },
}

/// Plan a restore by identifying conflicts between the current state and
/// the target checkpoint. Returns a list of conflicts that must be resolved
/// before restoration can proceed.
pub fn plan_restore() -> Vec<Conflict> {
    // Placeholder: the full implementation will compare the current state
    // with the target snapshot and enumerate conflicts.
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_restore_returns_empty_for_now() {
        assert!(plan_restore().is_empty());
    }
}
