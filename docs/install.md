# Installation

## Prerequisites

Varn requires **Rust 1.85+** (Rust 2024 edition). If you don't have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
```

## Option 1: Build from source (recommended)

```bash
git clone https://github.com/flawme/varn.git
cd varn
cargo build --release
```

The binary is at `target/release/varn`.

### Add to PATH

**Linux / macOS:**

```bash
sudo cp target/release/varn /usr/local/bin/
```

Or, without sudo, add to your user bin:

```bash
mkdir -p ~/.local/bin
cp target/release/varn ~/.local/bin/
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

For macOS with zsh:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

**Windows (PowerShell):**

```powershell
Copy-Item target\release\varn.exe "$env:USERPROFILE\.cargo\bin\"
```

Or copy to any directory already on your `PATH`.

## Option 2: Install with Cargo

```bash
cargo install --git https://github.com/flawme/varn.git
```

This downloads, builds, and installs `varn` to `~/.cargo/bin/` automatically.

## Option 3: One-liner (Linux / macOS)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y && \
source "$HOME/.cargo/env" && \
cargo install --git https://github.com/flawme/varn.git
```

This installs Rust if needed, then builds and installs Varn in one go.

## Verify installation

```bash
varn --version
```

Should output:

```
varn 0.1.0
```

## Quick start

```bash
cd /your/project
varn init
varn checkpoint "before changes"
# ... make changes ...
varn diff <checkpoint-id>
varn restore <checkpoint-id>
```

See the [README](../README.md) for usage or the [architecture docs](architecture.md) for internals.
