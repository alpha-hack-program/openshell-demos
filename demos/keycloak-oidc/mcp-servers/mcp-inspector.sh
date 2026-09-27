#!/usr/bin/env bash
set -euo pipefail
# Opens MCP Inspector against the MCP Gateway's public endpoint.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_ENV="$SCRIPT_DIR/../.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"

MCP_GATEWAY_URL="https://mcp.${CLUSTER_APPS_DOMAIN}/mcp"

echo "Opening MCP Inspector → ${MCP_GATEWAY_URL}"
npx @modelcontextprotocol/inspector "$MCP_GATEWAY_URL"
