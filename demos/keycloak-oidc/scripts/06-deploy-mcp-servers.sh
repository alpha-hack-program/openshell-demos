#!/usr/bin/env bash
set -euo pipefail
# Deploys two MCP servers, each expected to validate the caller's
# Keycloak-issued OAuth access token directly.
: "${OPENSHELL_NAMESPACE:?set in .env}"

HERE="$(dirname "$0")"

helm upgrade --install mcp-servers "$HERE/../mcp-servers" \
  --namespace "$OPENSHELL_NAMESPACE"

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
