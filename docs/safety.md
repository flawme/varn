# Safety Model

Varn treats restoration as a potentially destructive operation. This document describes the safety guarantees and the restore safety pipeline.

## Core guarantees

- Varn never silently performs destructive operations.
- Restoration is always explicit.
- Varn does not modify files outside `.varn/` during `init`.
- Varn coexists with Git and never modifies Git metadata.
- No network communication, no telemetry, no account required.

## Idempotent checkpointing

Checkpointing the same state twice does not duplicate or overwrite. The second checkpoint is a no-op, reported as `status: "unchanged"` in JSON mode.

## Safety checkpoint before restore

By default, `varn restore` creates a checkpoint of the current state before restoring. This safety checkpoint can be used to undo a failed or unwanted restore. Use `--no-safety` to skip this.

```
checkpoint A
      |
user/AI changes filesystem
      |
restore A
      |
safety checkpoint (current state captured)
      |
verify (does filesystem match A?)
```

## Restore safety pipeline

The restore process follows a strict four-phase model:

1. **Plan** — compare the target snapshot with the current filesystem state and identify every action needed, plus any conflicts.
2. **Confirm** — if conflicts exist, require explicit user confirmation (or the `--yes` flag).
3. **Execute** — perform the actions: restore file contents from the object store, delete unexpected files, recreate directories.
4. **Verify** — re-scan the filesystem and confirm it matches the snapshot.

## Conflict detection

A conflict means the current filesystem state differs from the snapshot in a way that would cause data loss during restore:

- **Modified**: a file was changed since the checkpoint and would be overwritten.
- **Unexpected**: a file exists now but not in the checkpoint, and would be deleted.

Conflicts require confirmation before proceeding.

## Security hardening

The restore engine includes several security measures discovered through adversarial testing:

- **Path traversal prevention**: all restore paths are validated to reject `..` components and absolute paths.
- **Symlink escape prevention**: before writing a file or creating a directory, the engine checks that no ancestor directory in the target path is a symlink. This prevents the CVE-2026-71556 / GHSA-9qw7-j9xw-fv9c class of attacks.
- **Pre-flight object check**: before modifying any files, the engine verifies all objects referenced by the plan exist in the store. This prevents partial restores.
- **Object content hash verification**: after reading content from the object store and before writing it to the filesystem, the engine recomputes the SHA-256 hash and compares it to the expected hash. This catches corrupted or tampered objects before they overwrite user data.
- **Metadata restoration**: file permissions (readonly), modification times, and Unix ownership (uid/gid) are restored alongside content.
- **Full verification**: `verify_restore()` checks kind, content hash, symlink target, readonly flag, and mtime.
- **Hard link safety**: `CreateHardLink` actions verify that the target is not a symlink, preventing inode aliasing of external files (CVE-2026-32232 / ZeptoClaw R3). Both the link path and target path are checked for symlink escape.
