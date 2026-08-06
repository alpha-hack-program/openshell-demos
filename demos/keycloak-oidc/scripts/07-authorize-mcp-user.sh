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

USER_ID="${1:?usage: $0 <user-id> <server-name>}"
SERVER_NAME="${2:?usage: $0 <user-id> <server-name>}"
: "${OPENSHELL_NAMESPACE:?set in .env}"
: "${KEYCLOAK_HOST:?set in .env}"
: "${KEYCLOAK_REALM:?set in .env}"
: "${KEYCLOAK_ADMIN_TOKEN:?export a Keycloak admin bearer token first}"

REQUIRED_ROLE="${SERVER_NAME}-user"
SANDBOX_NAME="demo-${USER_ID}"

KC_USER_ID=$(curl -sk \
  -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users?username=${USER_ID}&exact=true" \
  | python3 -c "import sys, json; u = json.load(sys.stdin); print(u[0]['id'] if u else '')")
[ -n "$KC_USER_ID" ] || { echo "No Keycloak user found for '${USER_ID}' in realm ${KEYCLOAK_REALM}"; exit 1; }

HAS_ROLE=$(curl -sk \
  -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users/${KC_USER_ID}/role-mappings/realm" \
  | python3 -c "
import sys, json
roles = [r['name'] for r in json.load(sys.stdin)]
print('yes' if '${REQUIRED_ROLE}' in roles else 'no')
")
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
