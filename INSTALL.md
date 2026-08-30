# Installation

## Option 1: Download prebuilt binary (recommended)

Prebuilt binaries are available on the [releases page](https://github.com/flawme/varn/releases).

### Linux (x86_64)

```bash
curl -L https://github.com/flawme/varn/releases/latest/download/varn-linux-x86_64 -o ~/.local/bin/varn
chmod +x ~/.local/bin/varn
```

If `~/.local/bin` is not on your PATH:

```bash
echo 'export PATH="$HOME/.local/bin:$PATH"' >> ~/.bashrc
source ~/.bashrc
```

### macOS (x86_64)

```bash
curl -L https://github.com/flawme/varn/releases/latest/download/varn-macos-x86_64 -o /usr/local/bin/varn
chmod +x /usr/local/bin/varn
```

If you get a permission denied, use `sudo` or install to `~/.local/bin` instead.

### Windows (x86_64)

```powershell
curl -L https://github.com/flawme/varn/releases/latest/download/varn-windows-x86_64.exe -o "$env:USERPROFILE\.cargo\bin\varn.exe"
```

Or download the file directly from the [releases page](https://github.com/flawme/varn/releases) and place it in a directory on your `PATH`.

### One-liner (Linux)

```bash
curl -L https://github.com/flawme/varn/releases/latest/download/varn-linux-x86_64 -o /usr/local/bin/varn && chmod +x /usr/local/bin/varn
```

### One-liner (macOS)

```bash
curl -L https://github.com/flawme/varn/releases/latest/download/varn-macos-x86_64 -o /usr/local/bin/varn && chmod +x /usr/local/bin/varn
```

## Option 2: Install with Cargo

```bash
cargo install --git https://github.com/flawme/varn.git
```

This downloads, builds, and installs `varn` to `~/.cargo/bin/` automatically. Requires **Rust 1.85+**.

If you don't have Rust installed:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source "$HOME/.cargo/env"
cargo install --git https://github.com/flawme/varn.git
```

## Option 3: Build from source

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

Or, without sudo:

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

See the [README](README.md) for usage or the [architecture docs](docs/architecture.md) for internals.
