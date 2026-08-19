# Varn

**Local state checkpointing and rollback system.**

Varn captures a known state of your local filesystem, lets you observe what changed, and safely restores a previous state.

It is designed primarily for **AI agents and automated tools** operating on a local machine, but is equally useful for humans who want to safely experiment with local changes.

## What Varn Is

A local-first tool that answers:

> "An automated process changed my local environment. What exactly changed, and can I safely return to the previous state?"

## What Varn Is Not

Varn is **not Git**. It does not implement branches, remotes, commits, rebases, merges, pull/push, or Git-compatible history. Varn complements Git — Git tracks project/source history; Varn protects local state.

## Current Status

The following is implemented:

- **`varn init`** — initializes a Varn repository (creates `.varn/` with storage layout and config)
- **`varn checkpoint`** — captures the current filesystem state with content-addressed storage and deduplication
- **`varn list`** — displays available checkpoints
- **`varn diff`** — compares the current state with a checkpoint (added, modified, deleted)
- **`varn restore`** — restores a checkpoint with conflict detection, confirmation, and post-restore verification
- **Safety checkpoint before restore** — automatically creates a checkpoint of the current state before restoring, so a failed or unwanted restore can be undone
- Repository discovery (search upward for `.varn/`)
- Core data models (checkpoint identity, filesystem entries, diff types)
- **Filesystem scanner** — recursive directory walker with SHA-256 content hashing, symlink awareness (target capture), and graceful error handling
- **Content-addressed object storage** — file contents stored by SHA-256 hash with deduplication and sharded directory layout
- **Snapshot persistence** — snapshots saved as JSON in `.varn/snapshots/`, with deterministic checkpoint IDs derived from content
- **Idempotent checkpointing** — checkpointing the same state twice does not duplicate or overwrite; the second checkpoint is a no-op
- **Restore safety model** — conflict detection (modified/unexpected files), explicit confirmation, post-restore verification, safety checkpoint before restore
- **Symlink restoration** — symlinks are scanned with their targets, checkpointed, and fully restored (including target changes as conflicts)
- **Garbage collection** — `varn gc` removes objects from the store that no snapshot references, with `--dry-run` support
- Platform abstraction layer (os_name, is_posix, is_readonly, create_symlink)
- `--json` flag for machine-readable output

All five MVP commands are implemented, plus garbage collection.

## Supported Platforms

Varn targets Linux, macOS, and Windows. Platform-specific code is isolated behind clean abstractions in the `platform` module.

## CLI Usage

```text
varn init [path]           Initialize Varn in a directory (default: current directory)
varn checkpoint <desc>     Capture the current filesystem state
varn list                  Display available checkpoints
varn diff <checkpoint>     Compare current state with a checkpoint
varn restore <checkpoint>  Restore a checkpoint (--yes to skip confirmation, --no-safety to skip safety checkpoint)
varn gc                    Remove unreferenced objects from the store (--dry-run to preview)
varn --json <command>      Emit machine-readable JSON output
```

## Storage Model

Varn stores its metadata in a `.varn/` directory at the root of the managed path:

```text
.varn/
├── config.json       Repository configuration
├── objects/          Content-addressed blobs (SHA-256, sharded by first 2 hex chars)
├── snapshots/        Snapshot JSON files (<checkpoint_id>.json)
└── index/            Fast lookups (future)
```

The storage format is versioned (`config.json` contains a `version` field) to support future migrations. File contents are stored as content-addressed blobs in `objects/`, keyed by their SHA-256 hash. Identical content is stored only once (deduplication). Snapshots reference these blobs by hash, so restoring a checkpoint retrieves the original content.

## Safety Guarantees

- Varn never silently performs destructive operations.
- Restoration is always explicit.
- Varn does not modify files outside `.varn/` during `init`.
- Varn coexists with Git and never modifies Git metadata.
- No network communication, no telemetry, no account required.
- **Safety checkpoint before restore**: by default, `varn restore` creates a checkpoint of the current state before restoring. This safety checkpoint can be used to undo a failed or unwanted restore. Use `--no-safety` to skip this.
- **Idempotent checkpointing**: checkpointing the same state twice does not duplicate or overwrite. The second checkpoint is a no-op, reported as `status: "unchanged"` in JSON mode.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build
```

## License

MIT OR Apache-2.0
