//! Snapshot engine: creating checkpoints and representing state.
//!
//! This module is a placeholder for the full snapshot engine. It defines
//! the public types that other modules will build on.

use crate::core::CheckpointMeta;
use crate::filesystem::TreeEntry;

/// A complete snapshot: checkpoint metadata plus the captured filesystem
/// state.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Metadata describing the checkpoint.
    pub meta: CheckpointMeta,
    /// The entries captured in this snapshot, in a canonical order.
    pub entries: Vec<TreeEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CheckpointId;
    use std::path::PathBuf;

    #[test]
    fn snapshot_holds_meta_and_entries() {
        let snap = Snapshot {
            meta: CheckpointMeta {
                id: CheckpointId("a91f".to_string()),
                description: "test".to_string(),
                created_at: 1,
                root: PathBuf::from("/tmp"),
            },
            entries: vec![],
        };
        assert_eq!(snap.meta.id.0, "a91f");
        assert!(snap.entries.is_empty());
    }
}
