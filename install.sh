#!/usr/bin/env bash
set -euo pipefail

REPO="arcat0v0/nano-assistant"
SOURCE="${NA_SOURCE:-auto}"
BASE_URL="${NA_BASE_URL:-}"
GITHUB_API_URL="${NA_GITHUB_API_URL:-https://api.github.com/repos/$REPO}"
GITEE_API_URL="${NA_GITEE_API_URL:-https://gitee.com/api/v5/repos/arcat00/nano-assistant}"
GITHUB_RELEASES_URL="${NA_GITHUB_RELEASES_URL:-https://github.com/$REPO/releases}"
GITEE_RELEASES_URL="${NA_GITEE_RELEASES_URL:-https://gitee.com/arcat00/nano-assistant/releases}"
COUNTRY_URL="${NA_COUNTRY_URL:-https://www.cloudflare.com/cdn-cgi/trace}"
CONNECT_TIMEOUT="${NA_CONNECT_TIMEOUT:-3}"
PROBE_TIMEOUT="${NA_PROBE_TIMEOUT:-4}"
DOWNLOAD_TIMEOUT="${NA_DOWNLOAD_TIMEOUT:-120}"
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

[[ "$VERSION" == latest || "$VERSION" =~ ^v?(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || fatal "invalid NA_VERSION: $VERSION"
for value in "$CONNECT_TIMEOUT" "$PROBE_TIMEOUT" "$DOWNLOAD_TIMEOUT"; do
    [[ "$value" =~ ^[1-9][0-9]*$ ]] || fatal "timeouts must be positive integers"
done
if [ -z "$BASE_URL" ]; then
    case "$SOURCE" in auto | github | gitee) ;; *) fatal "invalid NA_SOURCE: $SOURCE" ;; esac
    command -v jq > /dev/null 2>&1 || fatal "jq is required for GitHub/Gitee release metadata"
fi
for tool in sha256sum tar gzip install mktemp; do
    command -v "$tool" > /dev/null 2>&1 || fatal "$tool is required"
done

fetch() {
    local limit="${3:-$DOWNLOAD_TIMEOUT}"
    if command -v curl > /dev/null 2>&1; then
        curl -fsSL --proto '=http,https' --proto-redir '=http,https' \
            --connect-timeout "$CONNECT_TIMEOUT" --max-time "$limit" "$1" -o "$2"
    elif command -v wget > /dev/null 2>&1; then
        command -v timeout > /dev/null 2>&1 || fatal "timeout is required when using wget"
        timeout "$limit" wget -q --timeout="$CONNECT_TIMEOUT" --tries=1 -O "$2" "$1"
    else
        fatal "curl or wget is required"
    fi
}

TMP_DIR=$(mktemp -d)
STAGED_BINARY=""
trap 'rm -rf "$TMP_DIR"; if [ -n "$STAGED_BINARY" ]; then rm -f "$STAGED_BINARY"; fi' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

api_url() {
    case "$1" in github) printf '%s' "${GITHUB_API_URL%/}" ;; gitee) printf '%s' "${GITEE_API_URL%/}" ;; esac
}

release_url() {
    case "$1" in github) printf '%s' "${GITHUB_RELEASES_URL%/}" ;; gitee) printf '%s' "${GITEE_RELEASES_URL%/}" ;; esac
}

valid_manifest() {
    jq -e --arg tag "$1" --arg artifact "$ARTIFACT" '
        .schema == 1 and .tag == $tag and
        (.commit | type == "string" and test("^[0-9a-f]{40}$")) and
        (.assets | type == "object") and
        (.assets | keys) == (["install.sh"] +
            (["x86_64-linux-gnu", "x86_64-linux-musl", "aarch64-linux-musl"] |
                map("na-" + . + ".tar.gz") | map(., . + ".sha256")) | sort) and
        ([.assets[] | type == "string" and test("^[0-9a-f]{64}$")] | all) and
        (.assets[$artifact] != null)
    ' "$2" > /dev/null 2>&1
}

resolve_latest() {
    local source="$1" page=1 count tag
    local releases="$TMP_DIR/releases-$source.jsonl" response="$TMP_DIR/page-$source.json"
    : > "$releases"
    while :; do
        fetch "$(api_url "$source")/releases?per_page=100&page=$page" "$response" "$PROBE_TIMEOUT" || return 1
        jq -e 'type == "array"' "$response" > /dev/null 2>&1 || return 1
        jq -c '.[]' "$response" >> "$releases" || return 1
        count=$(jq 'length' "$response") || return 1
        [ "$count" -eq 100 ] || break
        [ "$page" -lt 20 ] || { printf 'error: too many releases to resolve latest safely\n' >&2; return 1; }
        page=$((page + 1))
    done
    jq -rs '
        map(select(.draft != true and .prerelease != true) |
            select(.tag_name | type == "string" and test("^v(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)\\.(0|[1-9][0-9]*)$"))) |
        sort_by(.tag_name | ltrimstr("v") | split(".") | map(tonumber)) | reverse | .[].tag_name
    ' "$releases" > "$TMP_DIR/tags-$source" || return 1
    while IFS= read -r tag; do
        if fetch "$(release_url "$source")/download/$tag/release-manifest.json" "$TMP_DIR/manifest.json" "$PROBE_TIMEOUT"; then
            valid_manifest "$tag" "$TMP_DIR/manifest.json" || fatal "invalid release manifest for $tag on $source"
            TAG="$tag"
            return 0
        fi
    done < "$TMP_DIR/tags-$source"
    rm -f "$TMP_DIR/manifest.json"
    return 1
}

download_pair() {
    local base="$1"
    rm -f "$TMP_DIR/$ARTIFACT" "$TMP_DIR/$ARTIFACT.sha256"
    fetch "$base/$ARTIFACT.sha256" "$TMP_DIR/$ARTIFACT.sha256" "$PROBE_TIMEOUT" || return 1
    fetch "$base/$ARTIFACT" "$TMP_DIR/$ARTIFACT" || return 1
}

TAG=""
if [ -n "$BASE_URL" ]; then
    SOURCE="custom"
    if [ "$VERSION" = latest ]; then
        DOWNLOAD_BASE="${BASE_URL%/}/latest/download"
    else
        TAG="v${VERSION#v}"
        DOWNLOAD_BASE="${BASE_URL%/}/download/$TAG"
    fi
    info "Downloading nano-assistant ($VERSION, linux-$ARCH) from custom source..."
    download_pair "$DOWNLOAD_BASE" || fatal "custom source download failed"
else
    if [ "$SOURCE" = auto ]; then
        COUNTRY="unknown"
        if fetch "$COUNTRY_URL" "$TMP_DIR/country" "$PROBE_TIMEOUT"; then
            COUNTRY=$(sed -n 's/^loc=\([A-Z][A-Z]\)$/\1/p' "$TMP_DIR/country" | head -n 1)
            COUNTRY="${COUNTRY:-unknown}"
        fi
        PRIMARY="github"
        [ "$COUNTRY" != CN ] || PRIMARY="gitee"
        info "Network exit: $COUNTRY; preferred source: $PRIMARY"
    else
        PRIMARY="$SOURCE"
    fi
    SECONDARY="gitee"
    [ "$PRIMARY" != gitee ] || SECONDARY="github"
    if [ "$VERSION" = latest ]; then
        if ! resolve_latest "$PRIMARY"; then
            [ "$SOURCE" = auto ] || fatal "no complete stable release available on $PRIMARY"
            info "Cannot resolve latest on $PRIMARY; trying $SECONDARY"
            resolve_latest "$SECONDARY" || fatal "no complete stable release available on either source"
            PRIMARY="$SECONDARY"
            SECONDARY="gitee"
            [ "$PRIMARY" != gitee ] || SECONDARY="github"
        fi
    else
        TAG="v${VERSION#v}"
        if fetch "$(release_url "$PRIMARY")/download/$TAG/release-manifest.json" "$TMP_DIR/manifest.json" "$PROBE_TIMEOUT"; then
            valid_manifest "$TAG" "$TMP_DIR/manifest.json" || fatal "invalid release manifest for $TAG"
        else
            rm -f "$TMP_DIR/manifest.json"
        fi
    fi
    info "Downloading nano-assistant ($TAG, linux-$ARCH) from $PRIMARY..."
    if ! download_pair "$(release_url "$PRIMARY")/download/$TAG"; then
        [ "$SOURCE" = auto ] || fatal "$PRIMARY download failed for $TAG"
        info "Download failed on $PRIMARY; trying $SECONDARY for the same version $TAG"
        download_pair "$(release_url "$SECONDARY")/download/$TAG" || fatal "both sources failed for $TAG"
        PRIMARY="$SECONDARY"
    fi
    info "Selected release: $TAG; source: $PRIMARY"
fi

CHECKSUM=$(awk -v name="$ARTIFACT" 'NF == 2 && ($2 == name || $2 == "*" name) {print $1}' "$TMP_DIR/$ARTIFACT.sha256")
[[ "$CHECKSUM" =~ ^[0-9a-fA-F]{64}$ ]] || fatal "invalid checksum file"
ACTUAL=$(sha256sum "$TMP_DIR/$ARTIFACT")
[ "${ACTUAL%% *}" = "${CHECKSUM,,}" ] || fatal "checksum verification failed"
if [ -f "$TMP_DIR/manifest.json" ]; then
    for name in "$ARTIFACT" "$ARTIFACT.sha256"; do
        EXPECTED=$(jq -r --arg name "$name" '.assets[$name]' "$TMP_DIR/manifest.json")
        ACTUAL=$(sha256sum "$TMP_DIR/$name")
        [ "${ACTUAL%% *}" = "$EXPECTED" ] || fatal "release manifest checksum verification failed: $name"
    done
fi

[ "$(tar tzf "$TMP_DIR/$ARTIFACT")" = na ] || fatal "archive must contain only the na executable"
tar xzf "$TMP_DIR/$ARTIFACT" --no-same-owner -C "$TMP_DIR"
if [ ! -f "$TMP_DIR/na" ] || [ -L "$TMP_DIR/na" ]; then
    fatal "invalid executable in archive"
fi
mkdir -p "$INSTALL_DIR"
STAGED_BINARY=$(mktemp "$INSTALL_DIR/.na.XXXXXX")
install -m 0755 "$TMP_DIR/na" "$STAGED_BINARY"
info "Verifying downloaded executable..."
VERSION_OUTPUT=$("$STAGED_BINARY" --version 2>&1) || fatal "downloaded binary failed to run; existing installation preserved"
if [ -n "$TAG" ]; then
    [ "${VERSION_OUTPUT##* }" = "${TAG#v}" ] || fatal "binary version does not match $TAG"
fi
mv -f "$STAGED_BINARY" "$INSTALL_DIR/na"
STAGED_BINARY=""
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
