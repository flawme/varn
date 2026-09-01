//! Shared helpers for the regression suites.

#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use varn::filesystem::{EntryKind, EntryMeta, Scanner, TreeEntry};
use varn::snapshot::SnapshotData;
use varn::storage::Repo;

/// A fresh Varn repository in a temp dir.
pub struct TestRepo {
    pub tmp: TempDir,
    pub repo: Repo,
}

impl TestRepo {
    pub fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let repo = Repo::init(tmp.path(), varn::platform::os_name()).unwrap();
        Self { tmp, repo }
    }

    pub fn root(&self) -> &Path {
        &self.repo.root
    }

    /// Write a file (creating parent directories).
    pub fn write(&self, rel: &str, content: &[u8]) -> PathBuf {
        let full = self.root().join(rel);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(&full, content).unwrap();
        full
    }

    /// Read a file's contents.
    pub fn read(&self, rel: &str) -> Vec<u8> {
        fs::read(self.root().join(rel)).unwrap()
    }

    /// Read a file as a lossy string (for assertions).
    pub fn read_str(&self, rel: &str) -> String {
        String::from_utf8_lossy(&self.read(rel)).into_owned()
    }

    /// Scan the managed tree.
    pub fn scan(&self) -> varn::filesystem::ScanResult {
        Scanner::new(&self.repo.root).scan().unwrap()
    }

    /// Scan with the current `.varnignore` applied.
    pub fn scan_with_ignore(&self) -> varn::filesystem::ScanResult {
        Scanner::with_ignore(&self.repo.root).scan().unwrap()
    }

    /// Create a checkpoint from the current scan (stores blobs + saves).
    pub fn checkpoint(&self, description: &str) -> SnapshotData {
        let scan = self.scan();
        self.checkpoint_from_scan(&scan, description)
    }

    /// Create a checkpoint from a pre-computed scan.
    pub fn checkpoint_from_scan(
        &self,
        scan: &varn::filesystem::ScanResult,
        description: &str,
    ) -> SnapshotData {
        let meta = varn::core::CheckpointMeta {
            id: varn::core::CheckpointId("pending".to_string()),
            description: description.to_string(),
            created_at: 1_000_000,
            root: self.repo.root.clone(),
        };
        let snapshot = SnapshotData::new(meta, scan.entries.clone());
        snapshot
            .store_content_blobs(&self.repo.root, &self.repo.object_store())
            .unwrap();
        snapshot.save(&self.repo.snapshots_dir()).unwrap();
        snapshot
    }

    /// Plan a restore of `snapshot` against the current disk state.
    pub fn plan_restore(&self, snapshot: &SnapshotData) -> varn::restore::RestorePlan {
        let current = self.scan();
        varn::restore::plan_restore(&snapshot.entries, &current.entries)
    }

    /// Execute a restore plan, merging plan warnings into the result
    /// (the same contract the CLI applies).
    pub fn execute(&self, plan: &varn::restore::RestorePlan) -> varn::restore::RestoreResult {
        let mut result =
            varn::restore::execute_restore(plan, &self.repo.root, &self.repo.object_store())
                .unwrap();
        result.warnings.extend(plan.warnings.clone());
        result
    }

    /// Full restore: plan + execute.
    pub fn restore(&self, snapshot: &SnapshotData) -> varn::restore::RestoreResult {
        let plan = self.plan_restore(snapshot);
        self.execute(&plan)
    }

    /// Whether post-restore verification passes.
    pub fn verifies(&self, snapshot: &SnapshotData) -> bool {
        varn::restore::verify_restore(&self.repo.root, &snapshot.entries)
    }
}

/// Set a file's mtime (whole seconds) — cross-platform via filetime.
pub fn set_mtime(path: &Path, unix_secs: i64) {
    filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(unix_secs, 0)).unwrap();
}

/// Read a file's mtime as unix seconds (same truncation as the scanner).
pub fn get_mtime(path: &Path) -> Option<i64> {
    fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// Read a file's sub-second mtime component in nanoseconds.
pub fn get_mtime_nanos(path: &Path) -> Option<u32> {
    fs::symlink_metadata(path)
        .ok()?
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.subsec_nanos())
}

/// Build a synthetic TreeEntry (for plan-level tests).
pub fn entry(path: &str, kind: EntryKind, hash: Option<&str>) -> TreeEntry {
    TreeEntry {
        path: PathBuf::from(path),
        meta: EntryMeta {
            kind,
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

/// Find a scanned entry by path (forward-slash normalized).
pub fn find_entry<'a>(scan: &'a varn::filesystem::ScanResult, rel: &str) -> &'a TreeEntry {
    scan.entries
        .iter()
        .find(|e| e.path.to_string_lossy().replace('\\', "/") == rel)
        .unwrap_or_else(|| panic!("entry {rel} not found in scan"))
}

/// Find a snapshot entry by path (forward-slash normalized).
pub fn find_snap_entry<'a>(snapshot: &'a SnapshotData, rel: &str) -> &'a TreeEntry {
    snapshot
        .entries
        .iter()
        .find(|e| e.path.to_string_lossy().replace('\\', "/") == rel)
        .unwrap_or_else(|| panic!("entry {rel} not found in snapshot"))
}

/// SHA-256 of a byte slice, first 12 hex chars (checkpoint-ID length).
pub fn short_hash(data: &[u8]) -> String {
    varn::filesystem::hash_bytes(data)[..12].to_string()
}
