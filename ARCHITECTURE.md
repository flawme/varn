# Varn Architecture

## Overview

Varn is a local state checkpointing and rollback system. This document describes the current architecture and the design decisions behind it.

## Module Structure

```text
src/
├── main.rs        Binary entry point
├── cli.rs         CLI: argument parsing, output formatting, exit codes
├── core.rs        Domain models: checkpoint identity, snapshot metadata
├── filesystem.rs  Filesystem data model + scanner: entry types, metadata, recursive scanning with SHA-256 hashing
├── snapshot.rs    Snapshot engine: creating checkpoints (placeholder)
├── storage.rs     On-disk layout, repository config, persistence
├── diff.rs        Diff engine: comparing two states
├── restore.rs     Restore engine: conflict detection, safe restore (placeholder)
├── platform.rs    OS-specific abstractions (os_name, is_posix, is_readonly)
└── error.rs       Unified error types
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
- **The `.varn/` directory at the scan root is skipped** so Varn's own metadata is never included in a snapshot.
- **Per-entry errors are collected as `ScanWarning`s** rather than aborting the scan. A single inaccessible file does not prevent scanning the rest of the tree.
- **Entries are sorted by path** for deterministic output.
- **Directories and symlinks are not hashed** — only regular file contents are hashed.

### Content-addressed storage

File contents are stored as content-addressed blobs in `.varn/objects/`, keyed by their SHA-256 hash:

```text
content → SHA-256 hash → blob in objects/<2-char shard>/<remaining hex>
```

This allows identical file contents to be stored only once (deduplication). The `ObjectStore` writes blobs atomically (temp file + rename) and skips storage if the blob already exists. Objects are sharded into a two-level directory structure (`ab/cdef...`) to avoid having too many files in a single directory.

### Snapshot persistence

Snapshots are persisted as JSON files in `.varn/snapshots/<checkpoint_id>.json`. Each snapshot contains:

- `CheckpointMeta` — id, description, timestamp, root path
- A sorted list of `TreeEntry` records — the captured filesystem state

Checkpoint IDs are deterministic: they are the first 12 hex characters of a SHA-256 hash computed from the snapshot's description, timestamp, root path, and all entry paths/metadata/hashes. This means the same filesystem state checkpointed with the same description and timestamp produces the same ID.

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

The `version` field enables future format migrations. The current version is `1`.

## Testing

- **Unit tests** live in each module under `#[cfg(test)]`.
- **Integration tests** live in `tests/` and exercise the public API end-to-end.
- Tests use `tempfile::TempDir` for isolation and do not assume Linux-specific behavior (with documented exceptions for symlinks).

## Future Work

1. Temporary safety checkpoint before restore
2. Storage-format migration support
3. Concurrent scanning for large directory trees
4. Full diff engine (metadata changes, permissions, symlink targets)
5. Garbage collection of unreferenced objects
6. Symlink and special file restoration
