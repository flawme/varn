# Installation

## Option 1: Install script (recommended)

The install script auto-detects your platform, downloads the binary, and adds it to your PATH:

```bash
curl -fsSL https://raw.githubusercontent.com/flawme/varn/main/install.sh | sh
```

To install a specific version:

```bash
curl -fsSL https://raw.githubusercontent.com/flawme/varn/main/install.sh | sh -s -- v0.2.0
```

Options:

| Flag | Description |
|------|-------------|
| `--bin-dir <path>` | Override the install directory |
| `--no-modify-path` | Do not modify shell config |
| `<version>` | Install a specific version (e.g. `v0.2.0`) |

**Supported platforms:** Linux and macOS on x86_64 and aarch64. Windows users should use Option 2 below.

## Option 2: Download prebuilt binary manually

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

## Option 3: Install with Cargo

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

## Option 4: Build from source

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
varn 0.2.0
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
