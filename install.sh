#!/bin/sh
# Build suprpull and install it to ~/.local/bin. No root needed.
#
#   curl -fsSL https://raw.githubusercontent.com/alpwrk/suprpull/main/install.sh | sh
#   curl -fsSL https://raw.githubusercontent.com/alpwrk/suprpull/main/install.sh | sh -s -- --uninstall
#
# Run from a git checkout (./install.sh) it builds that checkout instead of
# downloading the source.
set -eu

NAME=suprpull
REPO=alpwrk/suprpull
REF=${SUPRPULL_REF:-main}
MIN_RUST=1.85
BIN_DIR="$HOME/.local/bin"

info() { printf '\033[1;34m::\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

usage() {
    cat <<EOF
Usage: install.sh [--uninstall] [--help]

Builds $NAME in release mode and installs it to $BIN_DIR.
Outside a source checkout the source of github.com/$REPO ($REF) is downloaded.

Environment:
  SUPRPULL_REF   branch or tag to download (default: main)
EOF
}

# Succeeds when version $1 >= version $2 (major.minor only).
version_ge() {
    a_major=${1%%.*}; a_rest=${1#*.}; a_minor=${a_rest%%.*}
    b_major=${2%%.*}; b_rest=${2#*.}; b_minor=${b_rest%%.*}
    [ "$a_major" -gt "$b_major" ] || { [ "$a_major" -eq "$b_major" ] && [ "$a_minor" -ge "$b_minor" ]; }
}

uninstall() {
    target="$BIN_DIR/$NAME"
    if [ -e "$target" ]; then
        rm -f "$target"
        info "removed $target"
    else
        info "$target is not installed"
    fi
    info "config, history and cache were kept:"
    echo "   ~/.config/$NAME  ~/.local/share/$NAME  ~/.cache/$NAME"
}

# Prints the source directory: the script's own checkout, or a fresh download.
source_dir() {
    case "$0" in
        */install.sh|install.sh)
            dir=$(cd "$(dirname "$0")" && pwd)
            if grep -q "^name = \"$NAME\"" "$dir/Cargo.toml" 2>/dev/null; then
                echo "$dir"
                return
            fi ;;
    esac

    url="https://github.com/$REPO/archive/$REF.tar.gz"
    info "downloading $url" >&2
    archive="$TMP_DIR/source.tar.gz"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$archive" "$url" || die "download failed: $url"
    elif command -v wget >/dev/null 2>&1; then
        wget -qO "$archive" "$url" || die "download failed: $url"
    else
        die "neither curl nor wget found"
    fi
    tar -xzf "$archive" -C "$TMP_DIR" --strip-components=1 || die "could not unpack $archive"
    [ -f "$TMP_DIR/Cargo.toml" ] || die "download did not contain a Cargo.toml"
    echo "$TMP_DIR"
}

main() {
    case "${1:-}" in
        -h|--help) usage; exit 0 ;;
        --uninstall) uninstall; exit 0 ;;
        "") ;;
        *) usage >&2; exit 2 ;;
    esac

    command -v cargo >/dev/null 2>&1 \
        || die "cargo not found. Install Rust via https://rustup.rs and try again."

    rust_version=$(rustc --version | awk '{print $2}')
    version_ge "$rust_version" "$MIN_RUST" \
        || die "Rust $MIN_RUST or newer is required (found $rust_version). Run: rustup update"

    if ! command -v ffmpeg >/dev/null 2>&1; then
        warn "ffmpeg not found in \$PATH. $NAME needs it to download;"
        warn "install it or set ffmpeg_path in ~/.config/$NAME/config.toml."
    fi

    TMP_DIR=$(mktemp -d)
    trap 'rm -rf "$TMP_DIR"' EXIT INT TERM
    src=$(source_dir)

    info "building $NAME (release, rust $rust_version)"
    # A downloaded build gets its own target dir so nothing is left behind.
    if [ "$src" = "$TMP_DIR" ]; then
        CARGO_TARGET_DIR="$TMP_DIR/target"
    else
        CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$src/target}"
    fi
    export CARGO_TARGET_DIR
    cargo build --release --locked --manifest-path "$src/Cargo.toml"

    info "installing to $BIN_DIR/$NAME"
    mkdir -p "$BIN_DIR"
    install -m 755 "$CARGO_TARGET_DIR/release/$NAME" "$BIN_DIR/$NAME"

    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *) warn "$BIN_DIR is not in your \$PATH. Add this to your shell profile:"
           echo "   export PATH=\"$BIN_DIR:\$PATH\"" ;;
    esac

    info "done. Run '$NAME' to start."
}

# Everything runs from main, so a partially downloaded script does nothing.
main "$@"
