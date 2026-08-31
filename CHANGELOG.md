# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.0] - 2026-08-31

### Added

- **Cross-platform metadata parity** — Varn now captures and restores the
  same classes of state on Linux, macOS, and Windows:
  - **Full Unix permission mode** (rwx + setuid/setgid/sticky) is captured
    on Linux/macOS and fully restored, replacing the previous readonly-bit-
    only restore. Older snapshots without a mode fall back to readonly-bit
    behavior.
  - **Windows file attributes** (READONLY, HIDDEN, SYSTEM, ARCHIVE) are
    captured and restored via `SetFileAttributesW`.
  - **macOS BSD file flags** (`uchg`, `hidden`, ...) are captured via
    `lstat`/`st_flags` and restored via `lchflags`, best-effort: privileged
    flags are skipped with a warning, never failing the restore.
  - **Windows hard links** are detected via `GetFileInformationByHandle`
    (NTFS link counts) and restored with `CreateHardLink` semantics —
    previously Windows treated hard links as independent copies.
  - **Windows security descriptors** (owner, group, DACL) are captured in
    SDDL form and restored via `SetNamedSecurityInfoW`, best-effort.
- **Metadata drift detection** — restore now plans metadata-only updates
  when mode, flags, attributes, or ACL differ but content is unchanged
  (previously only readonly/mtime drift triggered an update).
- **Best-effort metadata warnings** — metadata that cannot be restored
  (privilege issues, unsupported filesystems) is reported as a warning in
  the restore result instead of being silently ignored or failing the
  operation.
- **Storage format backward compatibility** — snapshots written by older
  versions (without the new fields) deserialize unchanged; the new fields
  are optional with serde defaults.

### Changed

- `EntryMeta` schema extended with `mode`, `flags`, `attributes`, `acl`
  (all optional, omitted when not applicable to the platform).
- `RestoreAction::WriteFile`/`CreateDir` carry the new metadata fields.
- New target-gated dependencies: `libc` (macOS only), `windows-sys`
  (Windows only). No new dependencies on Linux builds.

## [0.1.1] - 2026-08-31

### Added

- **Ignore patterns** — `.varnignore` files with gitignore-style pattern matching (`*`, `**`, `?`, `[abc]`, `!negation`, directory-only, anchored). Loaded automatically by `varn checkpoint` and `varn diff`.
- **Incremental scanning** — persistent scan cache (`.varn/index/scan_cache.json`) records file size, mtime, and hash. Files whose size and mtime haven't changed reuse the cached hash instead of being re-read.
- **Hard link support** — hard links detected via `nlink` and content hash grouping. Primary file written normally; secondary files hard-linked to it during restore.
- **uid/gid restoration** — Unix ownership (uid/gid) captured during scan and restored via `chown` during restore (best-effort, requires root for non-owner changes).
- **Content streaming** — `store_content_blobs` streams file content in 64KB chunks instead of reading entire files into memory. Hash is computed during streaming and verified before committing.
- **`varn migrate` command** — storage format migration framework. Checks `version` field in `config.json` and applies registered migrations sequentially.
- **Git coexistence guard** — `varn init` creates `.varn/.gitignore` containing `*`, so Git ignores the entire object store even when the managed directory is a git repository. A blind `git add -A` no longer stages Varn's objects. Nothing outside `.varn/` is touched.
- **`varn init --gitignore`** — optional flag that adds `.varn/` to the enclosing repository's root `.gitignore` (created if missing, idempotent). Fails with an actionable error if the directory is not inside a git repository.
- **Unignored-store warning** — `varn init` and `varn checkpoint` warn (text and JSON `warnings`) when the store sits inside a git work tree and is not excluded from git; covers stores created before the guard existed.
- **Guard backfill in `varn migrate`** — re-running `varn migrate` on a legacy store adds the missing `.varn/.gitignore` guard; `--dry-run` reports without writing.

### Security

- **CVE-2026-32232 (ZeptoClaw) R3** — `CreateHardLink` target symlink bypass. Fixed by checking that the hard link target is not itself a symlink, preventing aliasing of external inodes.
- **Predictable temp file name** — `store_content_streaming` used a predictable `<hash>.tmp` temp file name. Fixed by suffixing with the process ID to prevent symlink-based temp file attacks.
- **Scan cache poisoning** — added `CACHE_VERSION` field to the scan cache. Caches with a mismatched version are discarded. The cache is advisory only and never affects correctness.
- **Object store staging prevention** — the store-level git guard prevents accidental staging/committing of the content-addressed object store via `git add -A` in a Varn-managed git repository.

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
