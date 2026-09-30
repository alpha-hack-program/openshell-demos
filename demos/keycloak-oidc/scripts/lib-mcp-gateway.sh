# Sourced by 14/15/16 when --gw is passed, to route a sandbox's MCP calls
# through the RHCL MCP Gateway (../mcp-gateway) instead of directly to each
# server's own port-8000 Envoy listener. Looks up the gateway's PUBLIC
# endpoint from the live MCPGatewayExtension CR's spec.publicHost, NOT
# spec.privateHost (the in-cluster Service DNS name) — confirmed live
# (sandbox268): pointing a sandbox at privateHost makes Claude Code's MCP
# OAuth client reject the connection outright with "Protected resource
# <public> does not match expected <private>", because the gateway's own
# oauthProtectedResource discovery document (RFC 9728) always advertises
# publicHost as the resource identifier — MCP OAuth clients correctly
# refuse a resource whose metadata doesn't match the URL they actually
# connected to. So the "in-cluster" endpoint is unusable for OAuth-flow
# clients regardless of network reachability; always use publicHost here.
#
# Usage: source scripts/lib-mcp-gateway.sh
#   MCP_GATEWAY_URL=$(mcp_gateway_url) || exit 1
#
# Reads (both optional, same defaults ../mcp-gateway/values.yaml ships):
#   MCP_GATEWAY_EXT_NAME (default mcp-gateway-ext)
#   MCP_GATEWAY_EXT_NAMESPACE (default mcp-gateway-system)

mcp_gateway_url() {
  local ext_name="${MCP_GATEWAY_EXT_NAME:-mcp-gateway-ext}"
  local ext_namespace="${MCP_GATEWAY_EXT_NAMESPACE:-mcp-gateway-system}"
  local host
  host=$(oc get mcpgatewayextension "$ext_name" -n "$ext_namespace" \
    -o jsonpath='{.spec.publicHost}' 2>/dev/null || true)
  if [[ -z "$host" ]]; then
    echo "mcp_gateway_url: mcpgatewayextension '${ext_name}' not found in namespace '${ext_namespace}' (or has no publicHost set) — deploy demos/keycloak-oidc/mcp-gateway first" >&2
    return 1
  fi
  echo "https://${host}"
}
