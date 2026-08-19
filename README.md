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

This is the initial foundation. The following is implemented:

- **`varn init`** — initializes a Varn repository (creates `.varn/` with storage layout and config)
- Repository discovery (search upward for `.varn/`)
- Core data models (checkpoint identity, filesystem entries, diff types)
- Platform abstraction layer
- `--json` flag for machine-readable output

The following commands are recognized but **not yet implemented**:

- `varn checkpoint`
- `varn list`
- `varn diff`
- `varn restore`

## Supported Platforms

Varn targets Linux, macOS, and Windows. Platform-specific code is isolated behind clean abstractions in the `platform` module.

## CLI Usage

```text
varn init [path]           Initialize Varn in a directory (default: current directory)
varn checkpoint <desc>     Capture the current filesystem state (not yet implemented)
varn list                  Display available checkpoints (not yet implemented)
varn diff <checkpoint>     Compare current state with a checkpoint (not yet implemented)
varn restore <checkpoint>  Restore a checkpoint (not yet implemented)
varn --json <command>      Emit machine-readable JSON output
```

## Storage Model

Varn stores its metadata in a `.varn/` directory at the root of the managed path:

```text
.varn/
├── config.json       Repository configuration
├── objects/          Content-addressed blobs (future)
├── snapshots/        Snapshot metadata (future)
└── index/            Fast lookups (future)
```

The storage format is versioned (`config.json` contains a `version` field) to support future migrations.

## Safety Guarantees

- Varn never silently performs destructive operations.
- Restoration is always explicit.
- Varn does not modify files outside `.varn/` during `init`.
- Varn coexists with Git and never modifies Git metadata.
- No network communication, no telemetry, no account required.

## Development

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build
```

## License

MIT OR Apache-2.0
