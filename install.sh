#!/bin/sh
# Varn installer — downloads a prebuilt binary and adds it to PATH.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/flawme/varn/main/install.sh | sh
#
# Or to install a specific version:
#   curl -fsSL https://raw.githubusercontent.com/flawme/varn/main/install.sh | sh -s -- v0.1.0
#
# Options:
#   --bin-dir <path>   Override install directory (default: auto-detected)
#   --no-modify-path   Do not add the install dir to PATH in shell config
#   -h, --help         Show this help message

set -eu

REPO="flawme/varn"
VERSION="latest"
BIN_DIR=""
MODIFY_PATH=1

# --- Parse arguments ---
while [ $# -gt 0 ]; do
    case "$1" in
        --bin-dir)
            BIN_DIR="$2"
            shift 2
            ;;
        --no-modify-path)
            MODIFY_PATH=0
            shift
            ;;
        -h|--help)
            cat <<'EOF'
Varn installer

Usage: install.sh [VERSION] [OPTIONS]

Arguments:
  VERSION             Version to install (default: latest)

Options:
  --bin-dir <path>    Override install directory
  --no-modify-path    Do not modify shell config
  -h, --help          Show this help
EOF
            exit 0
            ;;
        -*)
            echo "error: unknown option: $1"
            echo "Run 'install.sh --help' for usage."
            exit 1
            ;;
        *)
            VERSION="$1"
            shift
            ;;
    esac
done

# --- Detect platform ---
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
    Linux)  PLATFORM="linux" ;;
    Darwin) PLATFORM="macos" ;;
    *)
        echo "error: unsupported operating system: $OS"
        echo "Varn supports Linux, macOS, and Windows."
        echo "Windows users: download the .exe from https://github.com/$REPO/releases"
        exit 1
        ;;
esac

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *)
        echo "error: unsupported architecture: $ARCH"
        echo "Varn provides prebuilt binaries for x86_64 and aarch64."
        echo "Build from source: https://github.com/$REPO#build-from-source"
        exit 1
        ;;
esac

# --- Determine download URL ---
if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_URL="https://github.com/$REPO/releases/latest/download/varn-${PLATFORM}-${ARCH}"
else
    DOWNLOAD_URL="https://github.com/$REPO/releases/download/${VERSION}/varn-${PLATFORM}-${ARCH}"
fi

# --- Determine install directory ---
if [ -z "$BIN_DIR" ]; then
    # Prefer ~/.local/bin if it exists or can be created
    if [ -d "$HOME/.local/bin" ] || mkdir -p "$HOME/.local/bin" 2>/dev/null; then
        BIN_DIR="$HOME/.local/bin"
    elif [ -w "/usr/local/bin" ]; then
        BIN_DIR="/usr/local/bin"
    else
        BIN_DIR="$HOME/.local/bin"
        mkdir -p "$BIN_DIR" 2>/dev/null || true
    fi
fi

echo "Installing Varn ($VERSION) for $PLATFORM-$ARCH..."
echo "  install dir: $BIN_DIR"
echo "  download:    $DOWNLOAD_URL"

# --- Download ---
TARGET="$BIN_DIR/varn"

if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$DOWNLOAD_URL" -o "$TARGET" || {
        echo "error: download failed"
        echo "Check the URL: $DOWNLOAD_URL"
        exit 1
    }
elif command -v wget >/dev/null 2>&1; then
    wget -qO "$TARGET" "$DOWNLOAD_URL" || {
        echo "error: download failed"
        echo "Check the URL: $DOWNLOAD_URL"
        exit 1
    }
else
    echo "error: neither curl nor wget is installed"
    exit 1
fi

chmod +x "$TARGET"

# --- Verify ---
if "$TARGET" --version >/dev/null 2>&1; then
    VERSION_OUTPUT="$("$TARGET" --version)"
    echo ""
    echo "Varn installed successfully: $VERSION_OUTPUT"
else
    echo "warning: binary downloaded but may not be executable on this system"
    echo "  try running: $TARGET --version"
fi

# --- Add to PATH ---
if [ "$MODIFY_PATH" -eq 1 ]; then
    NEEDS_PATH=0

    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) NEEDS_PATH=1 ;;
    esac

    if [ "$NEEDS_PATH" -eq 1 ]; then
        SHELL_NAME="$(basename "$SHELL" 2>/dev/null || echo bash)"

        case "$SHELL_NAME" in
            zsh)
                RC_FILE="$HOME/.zshrc"
                ;;
            fish)
                RC_FILE="$HOME/.config/fish/config.fish"
                ;;
            *)
                RC_FILE="$HOME/.bashrc"
                # Also add to .profile for login shells
                if [ -f "$HOME/.profile" ]; then
                    if ! grep -q "$BIN_DIR" "$HOME/.profile" 2>/dev/null; then
                        printf '\n# Added by Varn installer\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$HOME/.profile"
                    fi
                fi
                ;;
        esac

        if [ -w "$HOME" ] && [ ! -w "$RC_FILE" ] || [ ! -f "$RC_FILE" ]; then
            if [ -w "$HOME" ]; then
                touch "$RC_FILE" 2>/dev/null || true
            fi
        fi

        if [ -w "$RC_FILE" ] 2>/dev/null; then
            if ! grep -q "$BIN_DIR" "$RC_FILE" 2>/dev/null; then
                printf '\n# Added by Varn installer\nexport PATH="%s:$PATH"\n' "$BIN_DIR" >> "$RC_FILE"
                echo "Added $BIN_DIR to PATH in $RC_FILE"
                echo "Restart your shell or run: source $RC_FILE"
            fi
        else
            echo "note: could not write to $RC_FILE (read-only filesystem)"
            echo "      add this line manually to your shell config:"
            echo "      export PATH=\"$BIN_DIR:\$PATH\""
        fi
    fi
fi

echo ""
echo "Verify: varn --version"
