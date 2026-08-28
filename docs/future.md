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
