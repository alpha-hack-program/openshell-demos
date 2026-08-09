#!/usr/bin/env bash
set -euo pipefail
# Confirms a user holds the Keycloak realm role required for one MCP server,
# then grants their sandbox policy permission to reach it.
#
# Usage: ./07-authorize-mcp-user.sh <user-id> <mcp-server-a|mcp-server-b>
#
# Assumes a sandbox named demo-<user-id> already exists — this script does
# not create one. Assumes the user was already onboarded via
# 03-onboard-user.sh, so a `user-<id>` provider already exists and injects
# their Keycloak access token as a Bearer header on outbound calls.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

USER_ID="${1:?usage: $0 <user-id> <server-name>}"
SERVER_NAME="${2:?usage: $0 <user-id> <server-name>}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"

if [[ -z "${KEYCLOAK_ADMIN_TOKEN:-}" ]]; then
  : "${KEYCLOAK_ADMIN_USER:?set KEYCLOAK_ADMIN_USER in .env or export KEYCLOAK_ADMIN_TOKEN}"
  : "${KEYCLOAK_ADMIN_PASSWORD:?set KEYCLOAK_ADMIN_PASSWORD in .env or export KEYCLOAK_ADMIN_TOKEN}"
  KEYCLOAK_ADMIN_TOKEN=$(curl -sk -X POST \
    "https://${KEYCLOAK_HOST}/realms/master/protocol/openid-connect/token" \
    -d "grant_type=password" \
    -d "client_id=admin-cli" \
    -d "username=${KEYCLOAK_ADMIN_USER}" \
    -d "password=${KEYCLOAK_ADMIN_PASSWORD}" \
    | jq -r '.access_token')
fi

REQUIRED_ROLE="${SERVER_NAME}-user"
SANDBOX_NAME="demo-${USER_ID}"

KC_USER_ID=$(curl -sk \
  -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users?username=${USER_ID}&exact=true" \
  | jq -r '.[0].id // empty')
[ -n "$KC_USER_ID" ] || { echo "No Keycloak user found for '${USER_ID}' in realm ${KEYCLOAK_REALM}"; exit 1; }

HAS_ROLE=$(curl -sk \
  -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users/${KC_USER_ID}/role-mappings/realm" \
  | jq -r --arg role "$REQUIRED_ROLE" '[.[].name] | if index($role) then "yes" else "no" end')
if [ "$HAS_ROLE" != "yes" ]; then
  echo "User '${USER_ID}' does not hold realm role '${REQUIRED_ROLE}'."
  echo "Grant it in Keycloak (realm ${KEYCLOAK_REALM}) before retrying."
  exit 1
fi

SERVER_PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$SERVER_NAME" -o jsonpath='{.spec.ports[0].port}')
openshell policy update "$SANDBOX_NAME" \
  --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${SERVER_PORT}:read-write:rest:enforce" \
  --binary /usr/bin/curl --wait

echo "User ${USER_ID} authorized for ${SERVER_NAME}."
echo "From inside sandbox ${SANDBOX_NAME}, calls to"
echo "  http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${SERVER_PORT}"
echo "will carry ${USER_ID}'s Keycloak access token as a Bearer header."
