#!/usr/bin/env bash
set -euo pipefail

REPO="arcat0v0/nano-assistant"
BASE_URL="${NA_BASE_URL:-https://github.com/$REPO/releases}"
VERSION="${NA_VERSION:-latest}"
INSTALL_DIR="${NA_INSTALL_DIR:-$HOME/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/nano-assistant"
CONFIG_FILE="$CONFIG_DIR/config.toml"

info() { printf '==> %s\n' "$*"; }
fatal() { printf 'error: %s\n' "$*" >&2; exit 1; }

[ "$(uname -s)" = "Linux" ] || fatal "this installer only supports Linux; see https://github.com/$REPO for other platforms"

case "$(uname -m)" in
    x86_64 | amd64) ARCH="x86_64" ;;
    aarch64 | arm64) ARCH="aarch64" ;;
    *) fatal "unsupported architecture: $(uname -m)" ;;
esac

ARTIFACT="na-$ARCH-linux-musl.tar.gz"

if [ "$VERSION" = "latest" ]; then
    DOWNLOAD_URL="$BASE_URL/latest/download/$ARTIFACT"
    CHECKSUM_URL="$BASE_URL/latest/download/$ARTIFACT.sha256"
else
    TAG="v${VERSION#v}"
    DOWNLOAD_URL="$BASE_URL/download/$TAG/$ARTIFACT"
    CHECKSUM_URL="$BASE_URL/download/$TAG/$ARTIFACT.sha256"
fi

fetch() {
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL "$1" -o "$2"
    elif command -v wget > /dev/null 2>&1; then
        wget -q -O "$2" "$1"
    else
        fatal "curl or wget is required"
    fi
}

TMP_DIR=$(mktemp -d)
trap 'rm -rf "$TMP_DIR"' EXIT

info "Downloading nano-assistant ($VERSION, linux-$ARCH)..."
fetch "$DOWNLOAD_URL" "$TMP_DIR/$ARTIFACT" || fatal "download failed: $DOWNLOAD_URL"
fetch "$CHECKSUM_URL" "$TMP_DIR/$ARTIFACT.sha256" || fatal "checksum download failed: $CHECKSUM_URL"

command -v sha256sum > /dev/null 2>&1 || fatal "sha256sum is required (install coreutils)"
(cd "$TMP_DIR" && sha256sum -c "$ARTIFACT.sha256" > /dev/null 2>&1) || fatal "checksum verification failed"

tar xzf "$TMP_DIR/$ARTIFACT" -C "$TMP_DIR"
mkdir -p "$INSTALL_DIR"
install -m 0755 "$TMP_DIR/na" "$INSTALL_DIR/na"
info "Installed $INSTALL_DIR/na"

mkdir -p "$CONFIG_DIR"
if [ ! -f "$CONFIG_FILE" ]; then
    cat > "$CONFIG_FILE" << 'EOF'
[provider]
provider = "openai"
model = "gpt-4o-mini"
api_key = ""  # Set your API key here or via NA_API_KEY env var

[memory]
enabled = true

[security]
mode = "confirm"  # direct | confirm | whitelist
whitelist = ["ls", "cat", "grep", "echo", "pwd", "cd"]

[behavior]
streaming = true
max_iterations = 10
EOF
    info "Created default config at $CONFIG_FILE"
fi

ensure_path() {
    case ":$PATH:" in
        *":$INSTALL_DIR:"*) return 0 ;;
    esac
    local line="export PATH=\"$INSTALL_DIR:\$PATH\""
    local configured=0
    for rc in "$HOME/.bashrc" "$HOME/.zshrc"; do
        [ -f "$rc" ] || continue
        if grep -qF "$INSTALL_DIR" "$rc"; then
            configured=1
        else
            printf '\n%s\n' "$line" >> "$rc"
            info "Added $INSTALL_DIR to PATH in $rc"
            configured=1
        fi
    done
    if [ "$configured" -eq 1 ]; then
        info "Restart your shell or run: $line"
    else
        info "Not on PATH, add manually: $line"
    fi
}
ensure_path

info "Verifying installation..."
"$INSTALL_DIR/na" --version > /dev/null 2>&1 || fatal "installed binary failed to run"
"$INSTALL_DIR/na" --version

printf '\n'
info "Done. Set your API key to get started:"
printf '  export NA_API_KEY="sk-..."\n'
printf '  na --config\n'
