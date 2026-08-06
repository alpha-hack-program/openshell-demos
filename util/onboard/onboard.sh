#!/usr/bin/env bash
# Thin wrapper: sources .env from the demo directory if present, then exec's
# the onboard binary.  Run from anywhere:
#   ./util/onboard/onboard.sh -u user2

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

DEMO_ENV="$REPO_ROOT/demos/keycloak-oidc/.env"
if [[ -f "$DEMO_ENV" ]]; then
    set -a
    # shellcheck source=/dev/null
    source "$DEMO_ENV"
    set +a
fi

BINARY="$SCRIPT_DIR/target/release/onboard"
if [[ ! -x "$BINARY" ]]; then
    echo "Binary not found at $BINARY — run 'cargo build --release' in $SCRIPT_DIR first." >&2
    exit 1
fi

exec "$BINARY" "$@"
