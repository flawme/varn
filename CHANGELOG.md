# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- **Ignore patterns** — `.varnignore` files with gitignore-style pattern matching (`*`, `**`, `?`, `[abc]`, `!negation`, directory-only, anchored). Loaded automatically by `varn checkpoint` and `varn diff`.
- **Incremental scanning** — persistent scan cache (`.varn/index/scan_cache.json`) records file size, mtime, and hash. Files whose size and mtime haven't changed reuse the cached hash instead of being re-read.
- **Hard link support** — hard links detected via `nlink` and content hash grouping. Primary file written normally; secondary files hard-linked to it during restore.
- **uid/gid restoration** — Unix ownership (uid/gid) captured during scan and restored via `chown` during restore (best-effort, requires root for non-owner changes).
- **Content streaming** — `store_content_blobs` streams file content in 64KB chunks instead of reading entire files into memory. Hash is computed during streaming and verified before committing.
- **`varn migrate` command** — storage format migration framework. Checks `version` field in `config.json` and applies registered migrations sequentially.

### Security

- **CVE-2026-32232 (ZeptoClaw) R3** — `CreateHardLink` target symlink bypass. Fixed by checking that the hard link target is not itself a symlink, preventing aliasing of external inodes.
- **Predictable temp file name** — `store_content_streaming` used a predictable `<hash>.tmp` temp file name. Fixed by suffixing with the process ID to prevent symlink-based temp file attacks.
- **Scan cache poisoning** — added `CACHE_VERSION` field to the scan cache. Caches with a mismatched version are discarded. The cache is advisory only and never affects correctness.

## [0.1.0] - 2026-08-28

### Added

- **`varn init`** — initializes a Varn repository (creates `.varn/` with storage layout and config)
- **`varn checkpoint`** — captures the current filesystem state with content-addressed storage and deduplication
- **`varn list`** — displays available checkpoints
- **`varn diff`** — compares the current state with a checkpoint (added, modified, deleted)
- **`varn restore`** — restores a checkpoint with conflict detection, confirmation, and post-restore verification
- **`varn gc`** — removes objects from the store that no snapshot references, with `--dry-run` support
- **`--json` flag** — machine-readable JSON output for all commands
- **Safety checkpoint before restore** — automatically creates a checkpoint of the current state before restoring
- **Symlink restoration** — symlinks are scanned with their targets, checkpointed, and fully restored
- **Permission and mtime restoration** — file permissions (readonly) and modification times are restored
- **Object content hash verification** — corrupted or tampered objects are detected before overwriting user data
- **Symlink escape prevention** — restore refuses to write through symlinks in the leading path
- **Pre-flight object check** — all objects are verified to exist before any filesystem modification
- Platform abstraction layer (Linux, macOS, Windows)
- Content-addressed object storage with SHA-256 hashing and sharded directory layout
- Idempotent checkpointing (same state checkpointed twice is a no-op)
- Repository discovery (search upward for `.varn/`)

### Security

- Path traversal prevention in restore paths
- Symlink escape prevention (CVE-2026-71556 / GHSA-9qw7-j9xw-fv9c class)
- Object content hash verification before writing (prevents silent data corruption)
- Full metadata verification in post-restore check (readonly, mtime)
- Atomic restore failure (pre-flight check prevents partial restores)
