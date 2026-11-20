#!/usr/bin/env bash
set -euo pipefail

BINARY_NAME="pks"
DEFAULT_DEST="$HOME/.local/bin"

usage() {
    echo "Usage: $0 [-d DEST_DIR] [-h]"
    echo ""
    echo "Build piki-sessions in release mode and install the binary."
    echo ""
    echo "Options:"
    echo "  -d DEST_DIR   Install directory (default: $DEFAULT_DEST)"
    echo "  -h            Show this help message"
}

DEST_DIR="$DEFAULT_DEST"

while getopts "d:h" opt; do
    case "$opt" in
        d) DEST_DIR="$OPTARG" ;;
        h) usage; exit 0 ;;
        *) usage; exit 1 ;;
    esac
done

# Anchor to the project root so build/cp paths are cwd-independent.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

echo "Building $BINARY_NAME in release mode..."
cargo build --release --bin "$BINARY_NAME"

mkdir -p "$DEST_DIR"
# Atomic replace via a temp file + rename, NOT an in-place `cp`. The persistent
# session daemon is this same binary re-invoked as `serve`; it keeps running
# after the app closes (that's the point — sessions survive), so the installed
# file is often still in use and an in-place overwrite fails with ETXTBSY
# ("Text file busy"). Renaming onto the path swaps in a fresh inode: the running
# daemon keeps its old (now-unlinked) inode, the next launch picks up the new
# binary, and no session has to be killed to upgrade.
install_binary() {
    local name="$1"
    local tmp="$DEST_DIR/.$name.new.$$"
    cp "target/release/$name" "$tmp"
    chmod +x "$tmp"
    mv -f "$tmp" "$DEST_DIR/$name"
    echo "Installed $name to $DEST_DIR/$name"
}
install_binary "$BINARY_NAME"

# Check if destination is in PATH
case ":$PATH:" in
    *":$DEST_DIR:"*) ;;
    *) echo "Warning: $DEST_DIR is not in your PATH" ;;
esac

echo "Done. Run '$BINARY_NAME' to start."
