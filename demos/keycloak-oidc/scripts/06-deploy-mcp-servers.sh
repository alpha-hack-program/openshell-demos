#!/usr/bin/env bash
set -euo pipefail
# Deploys two MCP servers, each expected to validate the caller's
# Keycloak-issued OAuth access token directly.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

# Source demo .env for OPENSHELL_NAMESPACE and MCP server image variables
DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"

helm upgrade --install mcp-servers "$DEMO_DIR/mcp-servers" \
  --namespace "$OPENSHELL_NAMESPACE" \
  --set "keycloak.issuer=https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}"

oc -n "$OPENSHELL_NAMESPACE" rollout status deployment/mcp-server-a
oc -n "$OPENSHELL_NAMESPACE" rollout status deployment/mcp-server-b

echo "MCP servers deployed. Reachable in-cluster at:"
for s in mcp-server-a mcp-server-b; do
  PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$s" -o jsonpath='{.spec.ports[0].port}')
  echo "  ${s}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${PORT}"
done
echo "Next: grant the relevant Keycloak realm role (mcp-server-a-user /"
echo "mcp-server-b-user) to a user, then run"
echo "  ./07-authorize-mcp-user.sh <user-id> <mcp-server-a|mcp-server-b>"
