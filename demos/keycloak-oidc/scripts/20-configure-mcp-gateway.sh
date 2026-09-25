#!/usr/bin/env bash
set -euo pipefail
# Configures the RHCL MCP Gateway (Kagenti/Kuadrant) so the mcp-servers
# chart's HTTPRoutes and MCPServerRegistrations work correctly.
#
# Prerequisites:
#   - RHCL operator installed (kagenti-operator v0.7.1+)
#   - Gateway CR and MCPGatewayExtension CR already created
#   - mcp-servers chart deployed with mcpGateway.enabled=true
#
# What this script does:
#   1. Removes the hostname from the Gateway's MCP listener so it accepts
#      HTTPRoutes with internal hostnames (e.g. mcp-compatibility.mcp.local).
#   2. Sets spec.publicHost on the MCPGatewayExtension (required when the
#      listener has no hostname).
#
# Safe to re-run (idempotent).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
: "${MCP_GATEWAY_NAME:=mcp-gateway}"
: "${MCP_GATEWAY_NAMESPACE:=openshift-ingress}"
: "${MCP_GATEWAY_EXT_NAME:=mcp-gateway-ext}"
: "${MCP_GATEWAY_EXT_NAMESPACE:=mcp-gateway-system}"
: "${MCP_GATEWAY_SECTION:=mcp}"
: "${MCP_GATEWAY_PUBLIC_HOST:=mcp.${CLUSTER_APPS_DOMAIN}}"

echo "=== Step 1: Remove hostname from Gateway listener '${MCP_GATEWAY_SECTION}' ==="
# Find the listener index for the MCP section
LISTENER_INDEX=$(oc get gateway "$MCP_GATEWAY_NAME" -n "$MCP_GATEWAY_NAMESPACE" \
  -o jsonpath='{range .spec.listeners[*]}{.name}{"\n"}{end}' \
  | grep -n "^${MCP_GATEWAY_SECTION}$" | cut -d: -f1)

if [[ -z "$LISTENER_INDEX" ]]; then
  echo "ERROR: listener '${MCP_GATEWAY_SECTION}' not found on Gateway ${MCP_GATEWAY_NAME}"
  exit 1
fi
# Convert 1-based grep output to 0-based JSON path
LISTENER_INDEX=$((LISTENER_INDEX - 1))

HAS_HOSTNAME=$(oc get gateway "$MCP_GATEWAY_NAME" -n "$MCP_GATEWAY_NAMESPACE" \
  -o jsonpath="{.spec.listeners[${LISTENER_INDEX}].hostname}" 2>/dev/null || true)

if [[ -n "$HAS_HOSTNAME" ]]; then
  echo "  Removing hostname '${HAS_HOSTNAME}' from listener index ${LISTENER_INDEX}..."
  oc patch gateway "$MCP_GATEWAY_NAME" -n "$MCP_GATEWAY_NAMESPACE" \
    --type='json' \
    -p="[{\"op\": \"remove\", \"path\": \"/spec/listeners/${LISTENER_INDEX}/hostname\"}]"
else
  echo "  Listener already has no hostname — nothing to do."
fi

echo ""
echo "=== Step 2: Set publicHost on MCPGatewayExtension ==="
CURRENT_PUBLIC=$(oc get mcpgatewayextension "$MCP_GATEWAY_EXT_NAME" -n "$MCP_GATEWAY_EXT_NAMESPACE" \
  -o jsonpath='{.spec.publicHost}' 2>/dev/null || true)

if [[ "$CURRENT_PUBLIC" == "$MCP_GATEWAY_PUBLIC_HOST" ]]; then
  echo "  publicHost already set to '${MCP_GATEWAY_PUBLIC_HOST}' — nothing to do."
else
  echo "  Setting publicHost to '${MCP_GATEWAY_PUBLIC_HOST}'..."
  oc patch mcpgatewayextension "$MCP_GATEWAY_EXT_NAME" -n "$MCP_GATEWAY_EXT_NAMESPACE" \
    --type='merge' \
    -p "{\"spec\":{\"publicHost\":\"${MCP_GATEWAY_PUBLIC_HOST}\"}}"
fi

echo ""
echo "=== Step 3: Verify ==="
echo "Waiting for MCPServerRegistrations to reconcile..."
sleep 5

oc get mcpserverregistrations -n "$MCP_GATEWAY_EXT_NAMESPACE" \
  -o custom-columns=NAME:.metadata.name,STATUS:.status.conditions[0].reason,MSG:.status.conditions[0].message \
  2>/dev/null || echo "(no MCPServerRegistrations found — deploy mcp-servers with mcpGateway.enabled=true first)"

echo ""
echo "Done. Test with:"
echo "  curl -sk https://${MCP_GATEWAY_PUBLIC_HOST}/mcp \\"
echo "    -H 'Content-Type: application/json' \\"
echo "    -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1.0\"}}}'"
