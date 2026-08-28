# Varn

**Local state checkpointing and rollback system.**

Varn captures a known state of your local filesystem, lets you observe what changed, and safely restores a previous state.

It is designed primarily for **AI agents and automated tools** operating on a local machine, but is equally useful for humans who want to safely experiment with local changes.

> "An automated process changed my local environment. What exactly changed, and can I safely return to the previous state?"

## What Varn Is Not

Varn is **not Git**. It does not implement branches, remotes, commits, rebases, merges, pull/push, or Git-compatible history. Varn complements Git — Git tracks project/source history; Varn protects local state.

## Quick start

```bash
varn init
varn checkpoint "before changes"
# ... make changes ...
varn diff <checkpoint-id>
varn restore <checkpoint-id>
```

See [installation instructions](docs/install.md) to get started.

## Commands

```text
varn init [path]           Initialize Varn in a directory
varn checkpoint <desc>     Capture the current filesystem state
varn list                  Display available checkpoints
varn diff <checkpoint>     Compare current state with a checkpoint
varn restore <checkpoint>  Restore a checkpoint
varn gc                    Remove unreferenced objects from the store
varn migrate               Migrate storage format to current version
varn --json <command>      Emit machine-readable JSON output
```

See the [CLI usage reference](docs/usage.md) for details.

## Features

- Content-addressed storage with SHA-256 hashing and deduplication
- Symlink scanning and full restoration
- Hard link detection and restoration (Unix)
- Permission, mtime, and uid/gid restoration (Unix)
- Conflict detection with explicit confirmation
- Safety checkpoint before restore (undo a bad restore)
- Idempotent checkpointing (same state = same ID, no duplicates)
- Incremental scanning with persistent mtime/size cache
- Content streaming for large files (no full file in memory)
- Ignore patterns via `.varnignore` (gitignore-style syntax)
- Storage format migration framework (`varn migrate`)
- Garbage collection with `--dry-run`
- `--json` output for AI agent integration
- Linux, macOS, and Windows support

## Limitations

No extended attributes (xattr), no ACL restoration, no concurrent scanning,
no streaming restore, no incremental restore. See
[Future Work](docs/future.md) for the full list.

## Documentation

- [Install](docs/install.md) — get Varn running
- [CLI usage](docs/usage.md) — command reference
- [Safety model](docs/safety.md) — guarantees and restore pipeline
- [Architecture](docs/architecture.md) — internals and design decisions
- [Future work](docs/future.md) — planned features and known limitations
- [Contributing](CONTRIBUTING.md) — how to contribute
- [Changelog](CHANGELOG.md) — version history

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

## License

MIT OR Apache-2.0
