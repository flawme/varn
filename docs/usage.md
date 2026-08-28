# CLI Usage

## Commands

### `varn init`

Initialize Varn in a directory.

```bash
varn init              # Initialize in current directory
varn init /path/to/dir # Initialize in a specific directory
```

Creates a `.varn/` directory with storage layout and config. Does not touch any existing files.

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

### `varn list`

Display available checkpoints.

```bash
varn list
```

Output:

```
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

```
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

## Global flags

### `--json`

Emit machine-readable JSON output. Works with all commands.

```bash
varn --json checkpoint "before changes"
varn --json list
varn --json diff a91f
varn --json restore a91f --yes
varn --json gc --dry-run
```

Errors are also emitted as JSON to stderr when `--json` is active, making Varn suitable for consumption by AI agents and automation tools.

## Checkpoint ID resolution

Checkpoint IDs can be specified by their full ID or any unique prefix:

```bash
varn diff a91f           # Prefix match
varn restore a91f3c2b    # Longer prefix
```

If a prefix matches multiple checkpoints, Varn reports an ambiguity error.
