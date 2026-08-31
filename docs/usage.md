# CLI Usage

## Commands

### `varn init`

Initialize Varn in a directory.

```bash
varn init              # Initialize in current directory
varn init /path/to/dir # Initialize in a specific directory
varn init --gitignore  # Also add .varn/ to the root .gitignore
```

Creates a `.varn/` directory with storage layout and config. Does not touch any existing files.

If the directory is inside a git repository, Varn also creates
`.varn/.gitignore` containing `*`, which makes Git ignore the entire store.
This protects against a blind `git add -A` staging tens of thousands of
content objects. Nothing outside `.varn/` is modified.

With `--gitignore`, Varn additionally appends `.varn/` to the enclosing
repository's root `.gitignore` (creating the file if it does not exist). The
entry is not duplicated if a recognized spelling (`.varn`, `.varn/`,
`/.varn`, `/.varn/`) is already present. The flag fails with an actionable
error if the directory is not inside a git repository.

If the store is not excluded from git (for example, a store created by an
older Varn version), `varn init` and `varn checkpoint` print a warning with
a copy-pasteable fix. Running `varn migrate` backfills the missing guard.

### `varn checkpoint`

Capture the current filesystem state.

```bash
varn checkpoint "before agent task"
```

A checkpoint includes:
- Unique ID (first 12 hex chars of a SHA-256 content hash)
- Timestamp
- Human-readable description
- Root path
- Full filesystem state (file paths, metadata, content hashes)

File contents are stored in the content-addressed object store with deduplication. Checkpointing the same state twice is a no-op (idempotent).

Incremental scanning: on each checkpoint, Varn loads `.varn/index/scan_cache.json` and reuses cached content hashes for files whose size and mtime haven't changed. The cache is saved after each scan.

### `varn list`

Display available checkpoints.

```bash
varn list
```

Output:

```text
ID             TIME                 DESCRIPTION
a91f3c2b4d5e   2026-08-19 20:14    before agent task
b72c1a3e5f7d   2026-08-19 20:27    after agent task
```

### `varn diff`

Compare the current state with a checkpoint.

```bash
varn diff a91f          # Use a checkpoint ID prefix
varn diff a91f3c2b4d5e  # Use a full checkpoint ID
```

Output:

```text
ADDED
  src/new_file.rs

MODIFIED
  src/main.rs

DELETED
  old_config.json
```

### `varn restore`

Restore a checkpoint.

```bash
varn restore a91f                          # Interactive (prompts on conflicts)
varn restore a91f --yes                    # Skip confirmation prompts
varn restore a91f --yes --no-safety        # Skip safety checkpoint too
```

By default, Varn creates a **safety checkpoint** of the current state before restoring, so a failed or unwanted restore can be undone. Use `--no-safety` to skip this.

If conflicts are detected (modified or unexpected files), Varn will:
1. List what will be overwritten or deleted
2. Ask for confirmation (unless `--yes` is passed)
3. Execute the restore
4. Verify the result matches the checkpoint

### `varn gc`

Remove objects from the store that no snapshot references.

```bash
varn gc             # Delete unreferenced objects
varn gc --dry-run   # Preview what would be deleted
```

Safe to run at any time. Objects referenced by any existing snapshot are always preserved.

### `varn migrate`

Migrate the storage format to the current version.

```bash
varn migrate             # Run pending migrations
varn migrate --dry-run   # Check if migration is needed
```

Reports the current and target storage versions. If the repository is already at the current version, no changes are made. If the repository is at a newer version than the installed Varn supports, an error is returned.

`varn migrate` also backfills the store-level git guard (`.varn/.gitignore`)
for stores created before it existed. `--dry-run` reports whether the guard
would be added without writing anything.

## Ignore patterns

Varn reads `.varnignore` files to exclude paths from checkpoints. The syntax follows gitignore conventions:

```text
# Comments and blank lines are ignored
*.log                    # Match by extension (any depth)
target/                  # Directory-only (trailing slash)
/build                   # Anchored to root (leading slash)
**/cache/                # Match at any depth
!important.log           # Negation (re-include)
```

Place a `.varnignore` file at the root of your Varn-managed directory. It is loaded automatically by `varn checkpoint` and `varn diff`.

### Pattern syntax

| Pattern | Matches |
|---------|---------|
| `*.log` | Any file ending in `.log`, at any depth |
| `target/` | A directory named `target` (and all its contents) |
| `/build` | A path named `build` at the root only |
| `**/cache/` | A directory named `cache` at any depth |
| `!important.log` | Re-includes a file previously excluded |
| `file[0-9].txt` | One character from the set `0-9` |
| `?` | Any single character except `/` |

## Git coexistence

Varn is designed to coexist with Git in the same directory:

- `varn init` creates `.varn/.gitignore` containing `*`, so Git ignores the
  entire store automatically. Nothing outside `.varn/` is modified.
- Varn never reads or writes Git metadata (`.git/`, index, refs).
- `varn checkpoint` skips `.varn/` during scans, so checkpointing a
  git-managed directory does not capture Git's internals.
- If the store is not excluded from git (legacy store), commands warn with a
  one-line fix: `echo '.varn/' >> .gitignore`. `varn init --gitignore`
  applies it for you; `varn migrate` backfills the store-level guard.

## Global flags

### `--json`

Emit machine-readable JSON output. Works with all commands.

```bash
varn --json checkpoint "before changes"
varn --json list
varn --json diff a91f
varn --json restore a91f --yes
varn --json gc --dry-run
varn --json migrate --dry-run
```

Errors are also emitted as JSON to stderr when `--json` is active, making Varn suitable for consumption by AI agents and automation tools.

### JSON output examples

**`varn --json checkpoint "test"`**

```json
{
  "status": "ok",
  "checkpoint_id": "a91f3c2b4d5e",
  "description": "test",
  "created_at": 1787162040,
  "root": "/project",
  "entries": 12,
  "saved": true,
  "warnings": []
}
```

**`varn --json list`**

```json
{
  "status": "ok",
  "checkpoints": [
    {
      "id": "a91f3c2b4d5e",
      "description": "before agent task",
      "created_at": 1787162040,
      "entries": 12
    }
  ]
}
```

**`varn --json diff a91f`**

```json
{
  "status": "ok",
  "checkpoint": "a91f3c2b4d5e",
  "changes": [
    { "kind": "added", "path": "src/new_file.rs" },
    { "kind": "modified", "path": "src/main.rs" },
    { "kind": "deleted", "path": "old_config.json" }
  ]
}
```

**`varn --json restore a91f --yes`**

```json
{
  "status": "ok",
  "checkpoint": "a91f3c2b4d5e",
  "safety_checkpoint": "b72c1a3e5f7d",
  "files_written": 3,
  "dirs_created": 1,
  "symlinks_created": 0,
  "deleted": 2,
  "verified": true,
  "warnings": []
}
```

**`varn --json gc --dry-run`**

```json
{
  "status": "ok",
  "dry_run": true,
  "total_objects": 45,
  "referenced_objects": 30,
  "deleted": 15,
  "deleted_hashes": ["aaaa1111", "bbbb2222"]
}
```

**Error output (stderr)**

```json
{
  "status": "error",
  "error": "checkpoint not found: xyz"
}
```

## Checkpoint ID resolution

Checkpoint IDs can be specified by their full ID or any unique prefix:

```bash
varn diff a91f           # Prefix match
varn restore a91f3c2b    # Longer prefix
```

If a prefix matches multiple checkpoints, Varn reports an ambiguity error.
