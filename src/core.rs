//! Core domain models: checkpoint identity and snapshot metadata.
//!
//! These types describe *what* a checkpoint is. The engine that creates
//! snapshots lives in the `snapshot` module.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A short, human-readable identifier for a checkpoint.
///
/// In the full implementation this will be derived from a hash of the
/// snapshot content. For now it is a placeholder type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct CheckpointId(pub String);

impl CheckpointId {
    /// Return the first `len` characters of the id, for compact display.
    ///
    /// Truncates at a UTF-8 character boundary so it never panics on
    /// non-ASCII content.
    pub fn short(&self, len: usize) -> &str {
        let s = self.0.as_str();
        if s.len() <= len {
            return s;
        }
        // Find the largest byte index <= len that is a char boundary.
        let mut end = len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Metadata describing a checkpoint, independent of the filesystem state
/// it captures.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckpointMeta {
    /// Unique identifier.
    pub id: CheckpointId,
    /// Human-readable description.
    pub description: String,
    /// Creation timestamp (seconds since the UNIX epoch).
    pub created_at: i64,
    /// Absolute path of the root that was checkpointed.
    pub root: PathBuf,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_id_short_truncates() {
        let id = CheckpointId("a91f3c2b".to_string());
        assert_eq!(id.short(4), "a91f");
        assert_eq!(id.short(8), "a91f3c2b");
        assert_eq!(id.short(100), "a91f3c2b");
    }

    #[test]
    fn checkpoint_id_short_handles_multibyte() {
        // "é" is two bytes; truncating at byte 1 must not panic.
        let id = CheckpointId("aé".to_string());
        assert_eq!(id.short(1), "a");
        // Byte 1 lands inside the two-byte "é"; must round down to "a".
        assert_eq!(id.short(2), "a");
        assert_eq!(id.short(3), "aé");
    }

    #[test]
    fn checkpoint_id_display() {
        let id = CheckpointId("a91f3c2b".to_string());
        assert_eq!(id.to_string(), "a91f3c2b");
    }

    #[test]
    fn checkpoint_meta_serialization_round_trip() {
        let meta = CheckpointMeta {
            id: CheckpointId("a91f3c2b".to_string()),
            description: "before agent task".to_string(),
            created_at: 1_700_000_000,
            root: PathBuf::from("/tmp/project"),
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: CheckpointMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(meta, back);
    }
}
