# Future Work

This page tracks planned features and known limitations of Varn that are **not**
in the v0.1.0 release. They are intentionally deferred, not forgotten.

Varn prioritizes correctness and safety over feature count. Each item below is
a candidate for a future release once its design and cross-platform behavior are
fully understood.

## Not yet implemented

- **No hard link support yet**
  Hard links are detected as regular files during a scan, but the link
  relationship between two paths is not recorded or restored. Restoring a
  checkpoint that contained hard links will produce independent copies instead
  of re-creating the links.

- **No incremental scanning (full scan every time)**
  Every `varn checkpoint` walks the entire managed tree from the root. There is
  no cached file index or mtime-based "what changed since last time" fast path,
  so large trees are re-scanned in full on each checkpoint.

- **No ignore patterns (like `.gitignore`)**
  Every file and directory under the managed root is captured. There is no
  mechanism to exclude paths (e.g. `target/`, `node_modules/`, build output)
  from a checkpoint.

- **No file ownership (uid/gid) restoration**
  File permissions (mode bits) are captured and restored where supported, but
  Unix ownership (uid/gid) is not. Restored files keep the uid/gid of the
  process performing the restore.

- **`store_content_blobs` reads entire files into memory**
  Content is read fully into a `Vec<u8>` before hashing and writing to the
  object store. There is no streaming path, so very large files are bounded by
  available memory rather than by disk bandwidth.

- **Storage format migration not implemented**
  The on-disk format carries a `version` field (currently `1`), but no
  migration path exists to upgrade an existing `.varn/` store from one version
  to the next. A future version change will require a migration tool.

## Planned directions

These are longer-term ideas from the project vision, not committed work:

- Incremental scanning with a persistent file index
- Configurable ignore patterns
- Hard link recording and restoration
- Content streaming for large files
- Storage-format version migration tooling
- File ownership (uid/gid) capture and restore where the platform supports it
- Protecting the restore operation itself with an automatic safety checkpoint
- Broader metadata capture (extended attributes, ACLs) where supported
