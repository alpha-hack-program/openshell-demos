#!/usr/bin/env bash
set -euo pipefail
# Path A extension (Keycloak-JWT validation, no SPIRE). Confirms a customer
# holds the Keycloak realm role required for one MCP server, then grants
# their sandbox policy permission to reach it. [VERIFY] every command here
# — new, untested design; see the demo README's Open Risks section.
#
# Usage: ./07-authorize-mcp-customer.sh <customer-id> <mcp-server-a|mcp-server-b>
#
# Assumes a sandbox named demo-<customer-id> already exists (see
# scripts/demo.sh) — this script does not create one. Assumes the customer
# was already onboarded via 03-onboard-customer.sh, so a `customer-<id>`
# provider already exists and injects their Keycloak access token as a
# Bearer header on outbound calls — the MCP server checks the realm role
# from that same token, so no new provider is created here, only a policy
# endpoint grant.

CUSTOMER_ID="${1:?usage: $0 <customer-id> <server-name>}"
SERVER_NAME="${2:?usage: $0 <customer-id> <server-name>}"
: "${OPENSHELL_NAMESPACE:?set in root .env}"
: "${KEYCLOAK_HOST:?set in .env}"
: "${KEYCLOAK_REALM:?set in .env}"
# Obtain this yourself first, e.g. an admin password-grant against the
# Keycloak master realm — kept out of this script per repo convention
# (same reasoning as the customer refresh-token login in
# 03-onboard-customer.sh: script your own login flow, don't hardcode one
# path here).
: "${KEYCLOAK_ADMIN_TOKEN:?export a Keycloak admin bearer token first}"

REQUIRED_ROLE="${SERVER_NAME}-user"
SANDBOX_NAME="demo-${CUSTOMER_ID}"

# 1. Confirm the customer actually holds the required Keycloak realm role.
#    This is a fail-fast check for a clearer error message — the real
#    enforcement point is the MCP server itself, which checks the same
#    claim on every request.
USER_ID=$(curl -sk \
  -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users?username=${CUSTOMER_ID}&exact=true" \
  | python3 -c "import sys, json; u = json.load(sys.stdin); print(u[0]['id'] if u else '')")
[ -n "$USER_ID" ] || { echo "No Keycloak user found for '${CUSTOMER_ID}' in realm ${KEYCLOAK_REALM}"; exit 1; }

HAS_ROLE=$(curl -sk \
  -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users/${USER_ID}/role-mappings/realm" \
  | python3 -c "
import sys, json
roles = [r['name'] for r in json.load(sys.stdin)]
print('yes' if '${REQUIRED_ROLE}' in roles else 'no')
")
if [ "$HAS_ROLE" != "yes" ]; then
  echo "Customer '${CUSTOMER_ID}' does not hold realm role '${REQUIRED_ROLE}'."
  echo "Grant it in Keycloak (realm ${KEYCLOAK_REALM}) before retrying."
  exit 1
fi

# 2. Allow this customer's sandbox to reach the MCP server over the
#    network. [VERIFY] the sandbox named demo-<customer-id> actually
#    exists (created via ./demo.sh <customer-id>) before running this.
SERVER_PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$SERVER_NAME" -o jsonpath='{.spec.ports[0].port}')
openshell policy update "$SANDBOX_NAME" \
  --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${SERVER_PORT}:read-write:rest:enforce" \
  --binary /usr/bin/curl --wait

echo "Customer ${CUSTOMER_ID} authorized for ${SERVER_NAME}."
echo "From inside sandbox ${SANDBOX_NAME}, calls to"
echo "  http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${SERVER_PORT}"
echo "will carry ${CUSTOMER_ID}'s Keycloak access token as a Bearer header."
