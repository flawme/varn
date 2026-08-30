# Future Work

Planned features and known limitations not yet in Varn.

## Limitations

- **No extended attributes (xattr) capture**
  Extended attributes (ACLs, security labels, custom metadata) are not captured or restored. This is platform-specific and requires careful design.

- **No ACL (Access Control List) restoration**
  POSIX ACLs and Windows ACLs are not captured. Only basic permission bits (readonly) and Unix ownership (uid/gid) are restored.

- **No concurrent scanning**
  Scanning is single-threaded. Large directory trees are walked sequentially.

- **No streaming restore**
  Restore reads entire objects from the object store into memory before writing. Very large files could benefit from streaming during restore.

- **No incremental restore**
  Restore re-writes all files, even if only a few changed. An incremental restore would only write files that differ from the current state.

- **No `.gitignore` integration**
  Varn reads `.varnignore` for ignore patterns but does not automatically respect `.gitignore` files. This may be added as an opt-in flag.

## Planned directions

- Extended attributes (xattr) capture and restoration
- ACL restoration (POSIX and Windows)
- Concurrent scanning for large directory trees
- Streaming restore for very large files
- Incremental restore (only write changed files)
- Protecting the restore operation itself with an automatic safety checkpoint
- Configurable ignore pattern sources (e.g. global ignore file)
- `.gitignore` integration (optionally respect `.gitignore` patterns)
