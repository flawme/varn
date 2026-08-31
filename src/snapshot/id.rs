//! Checkpoint ID generation and validation.

use crate::core::CheckpointMeta;
use crate::filesystem::TreeEntry;
use sha2::{Digest, Sha256};

/// Generate a deterministic checkpoint ID from the snapshot content.
///
/// The ID is the first 12 hex characters of the SHA-256 hash of the
/// serialized snapshot data (without the ID field, which is set after).
pub fn generate_checkpoint_id(meta: &CheckpointMeta, entries: &[TreeEntry]) -> String {
    let mut hasher = Sha256::new();
    // Hash the description, timestamp, and root for uniqueness.
    hasher.update(meta.description.as_bytes());
    hasher.update(meta.created_at.to_le_bytes());
    hasher.update(meta.root.to_string_lossy().as_bytes());
    // Hash each entry's path and metadata.
    for entry in entries {
        hasher.update(entry.path.to_string_lossy().as_bytes());
        hasher.update(entry.meta.hash.as_deref().unwrap_or("").as_bytes());
        hasher.update(entry.meta.size.to_le_bytes());
        hasher.update(match entry.meta.kind {
            crate::filesystem::EntryKind::File => b"file",
            crate::filesystem::EntryKind::Directory => b"dir\0",
            crate::filesystem::EntryKind::Symlink => b"syml",
            crate::filesystem::EntryKind::Other => b"othr",
        });
        // Include symlink target so different targets produce different IDs.
        if let Some(ref target) = entry.meta.target {
            hasher.update(target.to_string_lossy().as_bytes());
        }
    }
    let digest = hasher.finalize();
    format!("{:.12x}", digest)
}

/// Validate that a checkpoint ID is safe to use as a filename.
///
/// Checkpoint IDs are generated as lowercase hex strings. This function
/// rejects IDs that contain path separators, `..`, or other characters
/// that could escape the snapshots directory.
pub fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::CheckpointId;
    use crate::filesystem::{EntryKind, EntryMeta};
    use std::path::PathBuf;

    fn make_meta() -> CheckpointMeta {
        CheckpointMeta {
            id: CheckpointId("placeholder".to_string()),
            description: "test checkpoint".to_string(),
            created_at: 1_700_000_000,
            root: PathBuf::from("/tmp/project"),
        }
    }

    fn make_entry(path: &str, hash: Option<&str>) -> TreeEntry {
        TreeEntry {
            path: PathBuf::from(path),
            meta: EntryMeta {
                kind: EntryKind::File,
                size: 10,
                readonly: false,
                mtime: None,
                hash: hash.map(String::from),
                target: None,
                nlink: 1,
                hardlink_to: None,
                uid: None,
                gid: None,
                mode: None,
                flags: None,
                attributes: None,
                acl: None,
            },
        }
    }

    #[test]
    fn snapshot_data_generates_id_from_content() {
        let meta = make_meta();
        let entries = vec![make_entry("a.txt", Some("abc123"))];
        let id = generate_checkpoint_id(&meta, &entries);
        assert_eq!(id.len(), 12);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn snapshot_data_id_is_deterministic() {
        let meta = make_meta();
        let entries = vec![
            make_entry("a.txt", Some("abc123")),
            make_entry("b.txt", Some("def456")),
        ];
        let id1 = generate_checkpoint_id(&meta, &entries);
        let id2 = generate_checkpoint_id(&meta, &entries);
        assert_eq!(id1, id2);
    }

    #[test]
    fn snapshot_data_id_differs_for_different_content() {
        let meta = make_meta();
        let entries1 = vec![make_entry("a.txt", Some("abc123"))];
        let entries2 = vec![make_entry("a.txt", Some("xyz789"))];
        let id1 = generate_checkpoint_id(&meta, &entries1);
        let id2 = generate_checkpoint_id(&meta, &entries2);
        assert_ne!(id1, id2);
    }

    #[test]
    fn is_valid_id_accepts_hex() {
        assert!(is_valid_id("abcdef123456"));
        assert!(is_valid_id("ABCDEF"));
        assert!(is_valid_id("0123456789abcdef"));
    }

    #[test]
    fn is_valid_id_rejects_traversal() {
        assert!(!is_valid_id("../../../etc/passwd"));
        assert!(!is_valid_id(".."));
        assert!(!is_valid_id("/etc/passwd"));
        assert!(!is_valid_id("a/b"));
        assert!(!is_valid_id(""));
        assert!(!is_valid_id("hello world"));
    }
}
