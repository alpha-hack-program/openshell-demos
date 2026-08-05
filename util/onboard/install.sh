#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BINARY="$SCRIPT_DIR/target/release/onboard"

if [[ ! -f "$BINARY" ]]; then
    echo "Binary not found. Building..." >&2
    cargo build --release --manifest-path "$SCRIPT_DIR/Cargo.toml"
fi

mkdir -p ~/bin
install -m 755 "$BINARY" ~/bin/onboard
echo "Installed onboard to ~/bin/onboard"
