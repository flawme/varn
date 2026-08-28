# Contributing to Varn

Thanks for your interest in contributing to Varn. This document covers the basics.

## Development setup

```bash
git clone https://github.com/flawme/varn.git
cd varn
cargo build
cargo test
```

You need **Rust 1.85+** (Rust 2024 edition).

## Workflow

Before submitting a PR, make sure all four pass:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
cargo build --release
```

The project is warning-clean. No warnings are accepted in CI.

## Code style

### Do

- Write small, focused modules with clear ownership boundaries
- Use explicit types and meaningful error messages
- Add tests for every filesystem operation you add or change
- Keep platform-specific code in `src/platform.rs` — never scatter `#[cfg(target_os = ...)]` across core logic
- Handle errors properly — no `unwrap()` or `expect()` in production paths where errors can be handled
- Document safety invariants if `unsafe` is ever needed (it shouldn't be)

### Don't

- Add dependencies without a clear reason — every crate in `Cargo.toml` should justify its presence
- Add telemetry, network communication, or cloud features
- Assume POSIX semantics in cross-platform code
- Silently swallow errors or skip failing operations without warning
- Modify files outside the managed scope without a clearly documented reason
- Delete user data without an explicit restore operation

## Testing

- **Unit tests** live in each module under `#[cfg(test)]`.
- **Integration tests** live in `tests/` and exercise the public API end-to-end.
- Tests use `tempfile::TempDir` for isolation.
- Do not write tests that assume Linux-specific behavior unless the behavior genuinely differs by platform (use `#[cfg(unix)]` gates).

## Security

Varn may be used to protect important user data. If you find a security vulnerability:

1. Do not open a public issue.
2. Test it with an adversarial script first to confirm.
3. Fix it with a test that fails before the fix and passes after.
4. Document the vulnerability class in the commit message.

## Module structure

```text
src/
├── main.rs              Binary entry point
├── cli/                 CLI parsing, commands, formatting
├── core.rs              Domain models (checkpoint identity)
├── filesystem/          Scanner and entry types
├── snapshot/            Snapshot data and ID generation
├── storage/             Repo, object store, garbage collection
├── diff.rs              Diff engine
├── restore/             Plan, execute, verify
├── platform.rs          OS-specific abstractions
└── error.rs             Unified error types
```

See [docs/architecture.md](docs/architecture.md) for the full design rationale.

## Commit messages

Use clear, descriptive commit messages. Reference the issue number if applicable. Example:

```
Add hard link support to scanner

The scanner now detects hard links by comparing inode numbers
and stores them as a single content blob with multiple path
references.

Fixes #42
```

## Pull requests

1. Fork the repo and create a branch from `master`.
2. Make your changes following the guidelines above.
3. Ensure all four checks pass (fmt, clippy, test, build).
4. Write a clear PR description explaining what changed and why.
5. Keep PRs focused — one feature or fix per PR when possible.
