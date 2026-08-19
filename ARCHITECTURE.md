# Varn Architecture

## Overview

Varn is a local state checkpointing and rollback system. This document describes the current architecture and the design decisions behind it.

## Module Structure

```text
src/
├── main.rs        Binary entry point
├── cli.rs         CLI: argument parsing, output formatting, exit codes
├── core.rs        Domain models: checkpoint identity, snapshot metadata
├── filesystem.rs  Filesystem data model: entry types, metadata
├── snapshot.rs    Snapshot engine: creating checkpoints (placeholder)
├── storage.rs     On-disk layout, repository config, persistence
├── diff.rs        Diff engine: comparing two states
├── restore.rs     Restore engine: conflict detection, safe restore (placeholder)
├── platform.rs    OS-specific abstractions
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

### Content-addressed storage (planned)

The storage layer is designed toward content-addressed storage:

```text
content → hash → content object → snapshot references objects
```

This allows identical file contents to be stored only once (deduplication). The `objects/`, `snapshots/`, and `index/` directories are created during `init` but are not yet populated.

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

1. Filesystem scanning engine (concurrent, with content hashing)
2. Content-addressed object storage with deduplication
3. Snapshot creation and persistence
4. Full diff engine (metadata, permissions, symlinks)
5. Safe restore with conflict detection and confirmation
6. Temporary safety checkpoint before restore
7. Storage-format migration support
