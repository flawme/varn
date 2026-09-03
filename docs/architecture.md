# Varn Architecture

## Overview

Varn is a local state checkpointing and rollback system. This document describes the current architecture and the design decisions behind it.

## Module Structure

```text
src/
├── main.rs              Binary entry point
├── cli/
│   ├── mod.rs           CLI struct, argument parsing, command dispatch
│   ├── commands.rs      Command handlers (init, checkpoint, list, diff, restore, gc, migrate)
│   └── format.rs        Timestamp formatting, path absolutization
├── core.rs              Domain models: checkpoint identity, snapshot metadata
├── filesystem/
│   ├── mod.rs           Module re-exports
│   ├── types.rs         Entry types (EntryKind, EntryMeta, TreeEntry)
│   ├── scanner.rs       Recursive directory scanner with SHA-256 hashing
│   ├── ignore.rs        .varnignore pattern matching (gitignore-style)
│   └── scan_cache.rs    Incremental scan cache (mtime/size/hash persistence)
├── snapshot/
│   ├── mod.rs           Module re-exports
│   ├── data.rs          SnapshotData: persistence, content blob storage
│   └── id.rs            Checkpoint ID generation and validation
├── storage/
│   ├── mod.rs           Module re-exports
│   ├── repo.rs          Repo, RepoConfig, repository discovery
│   ├── object_store.rs  Content-addressed object storage with deduplication
│   ├── gc.rs            Garbage collection
│   └── migrate.rs       Storage format migration framework
├── diff.rs              Diff engine: comparing two states
├── restore/
│   ├── mod.rs           Module re-exports
│   ├── plan.rs          Restore plan types and planning logic
│   ├── execute.rs       Restore execution with safety checks
│   └── verify.rs        Post-restore verification
├── platform.rs          OS-specific abstractions (os_name, is_posix, is_readonly, create_symlink)
└── error.rs             Unified error types
```

## Design Principles

### Local-first

All operations are local. No network, no accounts, no telemetry.

### Cross-platform

Linux, macOS, and Windows are first-class targets. Platform-specific code is isolated in `platform.rs`. As platform-specific behavior grows, submodules (`unix`, `windows`) will be added behind `#[cfg]` gates. Core logic never contains `#[cfg(target_os = ...)]`.

### Safety first

- Restoration is treated as a potentially destructive operation.
- Varn never silently overwrites conflicting changes.
- All errors are actionable and include context.
- `init` only creates `.varn/` and never touches user files.
- The scanner never follows symlinks — it records them as symlinks.
- The scanner collects per-entry errors as warnings instead of aborting.

### Filesystem scanning

The `Scanner` recursively walks a directory tree and produces a sorted list of `TreeEntry` records with SHA-256 content hashes. Key design decisions:

- **`symlink_metadata`** is used so symlinks are recorded as symlinks, not followed. This prevents scanning outside the managed root through symlinks.
- **SHA-256 content hashing** for regular files enables deduplication and change detection.
- **Symlink targets are captured** via `read_link` and stored in `EntryMeta.target`. This enables full symlink restoration, including detecting when a symlink's target has changed.
- **The `.varn/` directory at the scan root is skipped** so Varn's own metadata is never included in a snapshot.
- **Ignore patterns** from `.varnignore` are applied during scanning. The pattern matcher supports gitignore-style syntax (`*`, `**`, `?`, `[abc]`, `!negation`, directory-only, anchored). Ignored directories are not recursed into.
- **Incremental scanning**: a persistent cache (`.varn/index/scan_cache.json`) records each file's size, full nanosecond mtime, and hash. Files whose fingerprint has not changed reuse the cached hash and existing object without being re-read. This is a warm-path performance optimization; as with every metadata-only cache, Varn assumes trusted local writers do not deliberately preserve the complete fingerprint while replacing content.
- **Hard link detection**: files with `nlink > 1` are grouped by content hash. The first file (by sorted path) in each group is the primary; others get `hardlink_to` set to the primary's path for link-based restoration.
- **Ownership capture**: on Unix, uid and gid are captured from inode metadata for restoration.
- **Per-entry errors are collected as `ScanWarning`s** rather than aborting the scan. A single inaccessible file does not prevent scanning the rest of the tree.
- **Entries are sorted by path** for deterministic output.
- **Directories and symlinks are not hashed** — only regular file contents are hashed.

### Content-addressed storage

File contents are stored as content-addressed blobs in `.varn/objects/`, keyed by their SHA-256 hash:

```text
content → SHA-256 hash → blob in objects/<2-char shard>/<remaining hex>
```

This allows identical file contents to be stored only once (deduplication). The `ObjectStore` writes blobs atomically (temp file + rename) and skips storage if the blob already exists. Objects are sharded into a two-level directory structure (`ab/cdef...`) to avoid having too many files in a single directory.

Content is streamed in 64KB chunks via `store_content_streaming`, which computes the SHA-256 hash during the write and verifies it before committing the object. This avoids reading entire large files into memory. Temp file names include the process ID to prevent predictable-path symlink attacks.

### Garbage collection

The `varn gc` command removes objects from the store that are not referenced by any snapshot. This prevents the object store from growing unboundedly as old checkpoints are deleted. The GC algorithm:

1. Loads all snapshots and collects the set of object hashes they reference (`SnapshotData::referenced_hashes`).
2. Lists all objects in the store (`ObjectStore::list_objects`).
3. Deletes objects not in the referenced set (`ObjectStore::delete_object`).

The `--dry-run` flag reports what would be deleted without actually deleting. GC is safe to run at any time — objects referenced by any existing snapshot are always preserved.

### Snapshot persistence

Snapshots are persisted as JSON files in `.varn/snapshots/<checkpoint_id>.json`. Each snapshot contains:

- `CheckpointMeta` — id, description, timestamp, root path
- A sorted list of `TreeEntry` records — the captured filesystem state

Checkpoint IDs are deterministic: they are the first 12 hex characters of a SHA-256 hash computed from the snapshot's description, root path, and all entry paths/metadata/hashes. The creation timestamp is deliberately excluded, so the same filesystem state checkpointed again with the same description is a true no-op.

**Idempotent checkpointing**: if a snapshot with the same ID already exists on disk, `save()` is a no-op and returns `false`. This prevents silent overwrites when two checkpoints of identical state are created within the same second. The CLI reports this as `status: "unchanged"` in JSON mode.

### Restore safety model

The restore process follows a strict four-phase safety model:

1. **Plan** — `plan_restore()` compares the target snapshot with the current filesystem state and produces actions (WriteFile, CreateDir, CreateSymlink, CreateHardLink, Delete) and conflicts (Modified, Unexpected).
2. **Confirm** — if conflicts exist, the user must confirm interactively (or pass `--yes`). In JSON mode, conflicts are reported and the command exits without making changes unless `--yes` is supplied.
3. **Safety checkpoint** — before executing the restore, Varn creates a checkpoint of the current state. This safety checkpoint is stored alongside regular checkpoints and is identifiable by its `[safety before restore of <id>]` description prefix. If the restore fails or produces unexpected results, the user can restore the safety checkpoint to recover. Use `--no-safety` to skip this.
4. **Execute + Verify** — `execute_restore()` performs the file operations (WriteFile, CreateDir, CreateSymlink, Delete), then `verify_restore()` re-scans the filesystem and confirms it matches the snapshot, including symlink targets, permissions, and timestamps.

### Security hardening

The restore engine includes several security measures discovered through adversarial testing:

- **Path traversal prevention**: all restore paths are validated to reject `..` components and absolute paths.
- **Symlink escape prevention**: before writing a file or creating a directory, the engine checks that no ancestor directory in the target path is a symlink. This prevents the CVE-2026-71556 / GHSA-9qw7-j9xw-fv9c class of attacks where a symlink in the leading path causes a write to escape the managed root.
- **Hard link target validation**: `CreateHardLink` actions verify that the target is not a symlink, preventing inode aliasing of external files (CVE-2026-32232 / ZeptoClaw R3).
- **Pre-flight object check**: before modifying any files, the engine verifies all objects referenced by the plan exist in the store. This prevents partial restores where some files are written and then a missing object aborts the rest.
- **Object content hash verification**: after reading content from the object store and before writing it to the filesystem, the engine recomputes the SHA-256 hash and compares it to the expected hash. This catches corrupted or tampered objects (bit rot, disk errors) before they overwrite user data.
- **Streaming content verification**: `store_content_streaming` computes the hash during the write and verifies it before committing the object. Temp file names include the process ID to prevent predictable-path symlink attacks.
- **Metadata restoration**: file permissions (readonly), modification times, and Unix ownership (uid/gid) are restored alongside content.
- **Full verification**: `verify_restore()` checks kind, content hash, symlink target, readonly flag, and mtime — not just content.
- **Scan cache integrity**: the incremental scan cache carries a version field; caches with a mismatched version are discarded. It uses size plus full nanosecond mtime to detect ordinary edits, including same-second NTFS writes, while avoiding a second complete read of already-stored content on warm checkpoints.

## Error Handling

All operations return `Result<T, VarnError>`. The `VarnError` enum distinguishes:

- I/O errors
- Serialization errors
- Already-initialized repositories
- Not-initialized repositories
- Invalid paths
- Not-yet-implemented features
- Other operational errors

Errors implement `Display` with actionable messages and `std::error::Error` with source chaining.

## CLI Design

The CLI is built with `clap` and supports a global `--json` flag. When `--json` is passed:

- Successful output is emitted as JSON to stdout.
- Errors are emitted as JSON to stderr.

This makes Varn suitable for consumption by other programs, including AI agents.

## Storage Format

The repository config is stored as `config.json`:

```json
{
  "version": 1,
  "root": "/path/to/project",
  "created_at": 1724080800,
  "platform": "linux"
}
```

The `version` field enables future format migrations. The current version is `1`. The `varn migrate` command checks the version and applies registered migrations sequentially. No migrations are registered yet; the framework is in place for future format changes.

## Testing

- **Unit tests** live in each module under `#[cfg(test)]`.
- **Integration tests** live in `tests/` and exercise the public API end-to-end.
- Tests use `tempfile::TempDir` for isolation and do not assume Linux-specific behavior (with documented exceptions for symlinks).

## Future Work

Planned features and known limitations are tracked in a dedicated page:

➡️ **[FUTURE.md](../FUTURE.md)**

Current limitations include: no extended attributes (xattr), no ACL restoration, no concurrent scanning, no streaming restore, and no incremental restore. See [`FUTURE.md`](../FUTURE.md) for details.
