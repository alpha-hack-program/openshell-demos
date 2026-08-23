#!/usr/bin/env bash
set -euo pipefail
# Deploys every MCP server listed in mcp-servers/values.yaml's servers:
# array, each expected to validate the caller's Keycloak-issued OAuth
# access token directly. Server names are read back from the rendered
# chart rather than hardcoded, so adding/removing entries in values.yaml
# doesn't require touching this script.

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

mapfile -t SERVERS < <(
  helm template mcp-servers "$DEMO_DIR/mcp-servers" \
    --set "keycloak.issuer=https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}" \
    --show-only templates/serviceaccount.yaml \
    | awk '/^  name: /{print $2}'
)

for s in "${SERVERS[@]}"; do
  oc -n "$OPENSHELL_NAMESPACE" rollout status "deployment/${s}"
done
oc -n "$OPENSHELL_NAMESPACE" rollout status deployment/mcp-postgres 2>/dev/null || true

echo "MCP servers deployed. Reachable in-cluster at:"
for s in "${SERVERS[@]}"; do
  PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$s" -o jsonpath='{.spec.ports[0].port}')
  echo "  ${s}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${PORT}"
done
echo "Next: grant the relevant Keycloak realm role (<server-name>-user) to a"
echo "user, then run"
echo "  ./07-authorize-mcp-user.sh <user-id> <server-name>"
