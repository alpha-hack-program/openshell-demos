#!/usr/bin/env bash
set -euo pipefail
# Opens MCP Inspector against the MCP Gateway's public endpoint,
# optionally authenticating as a Keycloak user.
#
# Usage:
#   ./mcp-inspector.sh                  # no auth (will get 401 if auth is enabled)
#   ./mcp-inspector.sh alice            # authenticate as alice
#   ./mcp-inspector.sh bob              # authenticate as bob
#   ./mcp-inspector.sh --cli alice      # CLI mode instead of web UI
#
# The password is assumed to match the username (demo-only credentials).
#
# Uses --protocol-era legacy to bypass MCP spec OAuth discovery (DCR).
# The MCP Gateway doesn't serve /.well-known/oauth-authorization-server yet,
# so modern/auto protocol era fails with a DCR error. Instead, this script
# obtains a Keycloak token directly and passes it via --header.
# See: docs on adding .well-known endpoint for full OAuth flow support.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_ENV="$SCRIPT_DIR/../.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${KEYCLOAK_CLIENT_ID_CLI:=openshell-cli}"

MCP_GATEWAY_URL="https://mcp.${CLUSTER_APPS_DOMAIN}/mcp"

MODE="--web"
USERNAME=""

for arg in "$@"; do
  case "$arg" in
    --cli) MODE="--cli" ;;
    --web) MODE="--web" ;;
    *)     USERNAME="$arg" ;;
  esac
done

HEADER_ARGS=()
if [[ -n "$USERNAME" ]]; then
  echo "Fetching token for user '${USERNAME}' from Keycloak..."
  TOKEN_RESPONSE=$(curl -sk -X POST \
    "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}/protocol/openid-connect/token" \
    -d "grant_type=password&client_id=${KEYCLOAK_CLIENT_ID_CLI}&username=${USERNAME}&password=${USERNAME}")

  TOKEN=$(echo "$TOKEN_RESPONSE" | jq -r .access_token)

  if [[ -z "$TOKEN" || "$TOKEN" == "null" ]]; then
    echo "ERROR: failed to obtain token for '${USERNAME}'" >&2
    echo "  Response: $(echo "$TOKEN_RESPONSE" | jq -r .error_description // .error // "unknown")" >&2
    exit 1
  fi

  echo "  Token obtained (${#TOKEN} chars, expires in $(echo "$TOKEN_RESPONSE" | jq -r .expires_in)s)"
  HEADER_ARGS=(--header "Authorization: Bearer ${TOKEN}")
fi

echo "Opening MCP Inspector (${MODE}) → ${MCP_GATEWAY_URL}"
[[ -n "$USERNAME" ]] && echo "  Authenticated as: ${USERNAME}"

npx @modelcontextprotocol/inspector "$MODE" \
  --transport http \
  --server-url "$MCP_GATEWAY_URL" \
  --protocol-era legacy \
  "${HEADER_ARGS[@]}"
