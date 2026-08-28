# Future Work

This page tracks planned features and known limitations of Varn that are **not**
in the current release. They are intentionally deferred, not forgotten.

Varn prioritizes correctness and safety over feature count. Each item below is
a candidate for a future release once its design and cross-platform behavior are
fully understood.

## Recently implemented

The following were previously listed as limitations and have now been
implemented:

- **Hard link support** — Hard links are detected during scan (via `nlink` and
  content hash grouping) and restored via `fs::hard_link`. The primary file in
  each group is written normally; secondary files are hard-linked to it.
- **Incremental scanning** — A persistent scan cache
  (`.varn/index/scan_cache.json`) records each file's size, mtime, and hash.
  Files whose size and mtime haven't changed reuse the cached hash instead of
  being re-read.
- **Ignore patterns** — `.varnignore` files with gitignore-style pattern
  matching (`*`, `**`, `?`, `[abc]`, `!negation`, directory-only, anchored).
  Loaded automatically by `varn checkpoint` and `varn diff`.
- **File ownership (uid/gid) restoration** — uid/gid are captured on Unix and
  restored via `chown` during restore. Best-effort: requires root to change to
  a different user; failures are silently ignored.
- **Content streaming** — `store_content_blobs` now streams file content in 64KB
  chunks instead of reading the entire file into memory. Hash is computed
  during streaming and verified before the object is committed.
- **Storage format migration** — A migration framework (`varn migrate`) checks
  the `version` field in `config.json` and applies registered migrations
  sequentially. No migrations are registered yet (version is still 1), but the
  framework is in place for future format changes.

## Security hardening

The following vulnerabilities were found during an adversarial security review
of the new features and have been patched:

- **CVE-2026-32232 (ZeptoClaw) R3 — Hard link target symlink bypass**:
  `CreateHardLink` checked the leading path for symlinks but did not verify
  that the hard link target itself was not a symlink. An attacker could craft a
  snapshot where the target is a symlink to an external file, causing
  `fs::hard_link` to alias an external inode. Fixed by checking the target's
  metadata and refusing if it is a symlink.
- **Predictable temp file name in `store_content_streaming`**:
  The streaming store used a predictable `<hash>.tmp` temp file name. An
  attacker who could predict the hash could pre-create a symlink at that path
  pointing to a privileged file. Fixed by suffixing the temp file name with the
  process ID.
- **Scan cache poisoning**:
  The scan cache had no version field, so a future format change would silently
  use an incompatible cache. Added a `CACHE_VERSION` field; caches with a
  mismatched version are treated as empty. The trust model is documented: the
  cache is advisory only and never affects correctness (content is
  independently hash-verified during storage).

## Not yet implemented

- **No extended attributes (xattr) capture**
  Extended attributes (ACLs, security labels, custom metadata) are not
  captured or restored. This is platform-specific and requires careful design.

- **No ACL (Access Control List) restoration**
  POSIX ACLs and Windows ACLs are not captured. Only basic permission bits
  (readonly) and Unix ownership (uid/gid) are restored.

- **No concurrent scanning**
  Scanning is single-threaded. Large directory trees are walked sequentially.
  Concurrent scanning would speed up checkpointing on multi-core systems.

- **No streaming restore**
  Restore reads entire objects from the object store into memory before
  writing. Very large files could benefit from streaming during restore.

- **No incremental restore**
  Restore always re-writes all files, even if only a few changed. An
  incremental restore would only write files that differ from the current
  state.

## Planned directions

These are longer-term ideas from the project vision, not committed work:

- Extended attributes (xattr) capture and restoration
- ACL restoration (POSIX and Windows)
- Concurrent scanning for large directory trees
- Streaming restore for very large files
- Incremental restore (only write changed files)
- Protecting the restore operation itself with an automatic safety checkpoint
- Configurable ignore pattern sources (e.g. global ignore file)
- `.gitignore` integration (optionally respect `.gitignore` patterns)
