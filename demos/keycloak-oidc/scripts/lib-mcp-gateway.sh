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
#
# Fails fast with a distinct message for each of: `oc` not logged in,
# the `oc get` call itself erroring (RBAC denial, wrong namespace, missing
# CR), and the CR existing but spec.publicHost being unset — so the most
# common case (an expired login) is never mistaken for "go deploy
# mcp-gateway" again.

mcp_gateway_url() {
  local ext_name="${MCP_GATEWAY_EXT_NAME:-mcp-gateway-ext}"
  local ext_namespace="${MCP_GATEWAY_EXT_NAMESPACE:-mcp-gateway-system}"
  local host err

  # `oc get ... 2>/dev/null || true` used to swallow EVERY failure mode
  # (expired/missing login, RBAC denial, genuinely-missing CR) into the
  # same generic "not found" message below — confirmed live (sandbox341):
  # an expired `oc login` produces that exact message, which sent us
  # looking for a deleted MCPGatewayExtension that was actually fine the
  # whole time. Check login state up front so the common case gets its
  # own clear, actionable error instead.
  if ! err=$( { oc whoami >/dev/null; } 2>&1 ); then
    echo "mcp_gateway_url: 'oc' is not logged in (oc whoami failed: ${err}) — run 'oc login' against this cluster, then retry" >&2
    return 1
  fi

  if ! host=$(oc get mcpgatewayextension "$ext_name" -n "$ext_namespace" \
    -o jsonpath='{.spec.publicHost}' 2>&1); then
    echo "mcp_gateway_url: 'oc get mcpgatewayextension ${ext_name} -n ${ext_namespace}' failed: ${host}" >&2
    return 1
  fi
  if [[ -z "$host" ]]; then
    echo "mcp_gateway_url: mcpgatewayextension '${ext_name}' not found in namespace '${ext_namespace}' (or has no publicHost set) — deploy demos/keycloak-oidc/mcp-gateway first" >&2
    return 1
  fi
  echo "https://${host}"
}
