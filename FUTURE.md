# Future Work

Planned features and known limitations not yet in Varn.

## Limitations

- **No extended attributes (xattr) capture**
  Extended attributes beyond ACLs (security labels like SELinux, custom
  user xattrs) are not captured or restored. Platform-specific and requires
  careful design.

- **No POSIX ACL capture (Linux)**
  Windows security descriptors are captured and restored (SDDL form), but
  Linux POSIX ACLs (`getfacl`/`setfacl`) are not. Files with extended ACL
  entries restore with their base mode only.

- **No concurrent scanning**
  Scanning is single-threaded. Large directory trees are walked sequentially.

- **No streaming restore**
  Restore reads entire objects from the object store into memory before
  writing. Very large files could benefit from streaming during restore.

- **No incremental restore**
  Restore re-writes all files, even if only a few changed. An incremental
  restore would only write files that differ from the current state.

- **No `.gitignore` integration**
  Varn reads `.varnignore` for ignore patterns but does not automatically
  respect `.gitignore` files. This may be added as an opt-in flag.

- **Junctions recorded as symlinks (Windows)**
  NTFS junctions are captured as `EntryKind::Symlink` with their target,
  and are never followed by the scanner (a junction pointing outside the
  root cannot cause escape). Distinguishing junctions from symlinks via
  reparse tags — and restoring junctions as junctions rather than
  symlinks — is future work.

- **Windows hard links require same volume**
  Hard link restoration on Windows falls back to an independent copy when
  the primary file and the link land on different volumes (NTFS hard links
  cannot cross volumes). A warning is emitted in that case.

- **macOS privileged BSD flags are best-effort**
  System-level flags (e.g. `schg`) require root to restore; unprivileged
  restores skip them with a warning.

## Platform parity

Varn aims for feature parity across Linux, macOS, and Windows. Current
state per platform:

| Feature | Linux | macOS | Windows |
|---------|-------|-------|---------|
| Checkpoint / diff / restore / gc / migrate | yes | yes | yes |
| Symlinks (scan + restore) | yes | yes | yes (Developer Mode/admin for creation) |
| Full permission mode | yes | yes | n/a (attributes instead) |
| File attributes | n/a | n/a | yes (READONLY, HIDDEN, SYSTEM, ARCHIVE) |
| BSD file flags | n/a | yes (best-effort) | n/a |
| Hard link detection + restore | yes | yes | yes (NTFS; same-volume) |
| Ownership (uid/gid) | yes | yes | n/a |
| Security descriptor (owner + DACL, SDDL) | n/a | n/a | yes (best-effort) |
| mtime restoration | yes | yes | yes |

## Planned directions

- POSIX ACL capture and restoration (Linux)
- Extended attributes (xattr) capture and restoration
- Concurrent scanning for large directory trees
- Streaming restore for very large files
- Incremental restore (only write changed files)
- Configurable ignore pattern sources (e.g. global ignore file)
- `.gitignore` integration (optionally respect `.gitignore` patterns)
