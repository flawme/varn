//! Filesystem data model.
//!
//! Types describing entries discovered while scanning a directory tree.
//! These are the building blocks for snapshots and diffs.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// The kind of a filesystem entry.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    /// A regular file.
    File,
    /// A directory.
    Directory,
    /// A symbolic link.
    Symlink,
    /// Any other entry type (sockets, fifos, block/char devices).
    Other,
}

impl EntryKind {
    /// Map a [`std::fs::FileType`] to an [`EntryKind`].
    pub fn from_file_type(ft: fs::FileType) -> Self {
        if ft.is_file() {
            Self::File
        } else if ft.is_dir() {
            Self::Directory
        } else if ft.is_symlink() {
            Self::Symlink
        } else {
            Self::Other
        }
    }
}

/// Metadata for a single filesystem entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntryMeta {
    /// Kind of entry (file, directory, symlink, other).
    pub kind: EntryKind,
    /// File size in bytes (0 for directories and symlinks).
    pub size: u64,
    /// Whether the entry is read-only (no write permission for owner).
    pub readonly: bool,
    /// Modification time as seconds since the UNIX epoch, if available.
    pub mtime: Option<i64>,
}

/// A single entry in a scanned tree, relative to the scan root.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TreeEntry {
    /// Path relative to the scan root, using forward slashes.
    pub path: PathBuf,
    /// Metadata for the entry.
    pub meta: EntryMeta,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn entry_kind_from_file_type_file() {
        let tmp = TempDir::new().unwrap();
        let f = tmp.path().join("f.txt");
        fs::write(&f, b"hello").unwrap();
        let meta = fs::symlink_metadata(&f).unwrap();
        let kind = EntryKind::from_file_type(meta.file_type());
        assert_eq!(kind, EntryKind::File);
    }

    #[test]
    fn entry_kind_from_file_type_dir() {
        let tmp = TempDir::new().unwrap();
        let meta = fs::symlink_metadata(tmp.path()).unwrap();
        let kind = EntryKind::from_file_type(meta.file_type());
        assert_eq!(kind, EntryKind::Directory);
    }

    #[test]
    fn entry_kind_from_file_type_symlink() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("target");
        fs::write(&target, b"x").unwrap();
        let link = tmp.path().join("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&target, &link).unwrap();
        #[cfg(not(unix))]
        {
            // Symlinks require privileges on Windows; skip the assertion there.
            return;
        }
        let meta = fs::symlink_metadata(&link).unwrap();
        let kind = EntryKind::from_file_type(meta.file_type());
        assert_eq!(kind, EntryKind::Symlink);
    }

    #[test]
    fn tree_entry_serialization_round_trip() {
        let entry = TreeEntry {
            path: PathBuf::from("src/main.rs"),
            meta: EntryMeta {
                kind: EntryKind::File,
                size: 42,
                readonly: false,
                mtime: Some(1_700_000_000),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: TreeEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn entry_kind_serializes_lowercase() {
        let json = serde_json::to_string(&EntryKind::Directory).unwrap();
        assert_eq!(json, r#""directory""#);
    }
}
