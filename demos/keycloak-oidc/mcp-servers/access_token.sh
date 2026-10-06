#!/usr/bin/env bash
set -euo pipefail

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

  echo "Token obtained:" && echo "${TOKEN}" && echo "Expires in $(echo "$TOKEN_RESPONSE" | jq -r .expires_in)s)"
fi

[[ -n "$USERNAME" ]] && echo "Authenticated as: ${USERNAME}"

