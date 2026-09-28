#!/usr/bin/env bash
set -euo pipefail
# Protects the RHCL MCP Gateway with JWT authentication AND fine-grained
# per-tool authorization using two Kuadrant AuthPolicy CRs — one per listener.
#
# Architecture (RHCL 1.4, Register PDF Ch.4-5):
#
#   Two Gateway listeners on the SAME port (8080):
#
#     mcp  (with hostname) — receives external traffic
#       → authn AuthPolicy: JWT validation, 401 on failure
#       → ext_proc parses JSON-RPC body, sets x-mcp-* headers,
#         changes :authority to internal hostname, clears route cache
#       → Envoy re-evaluates → matches per-server route on mcps listener
#
#     mcps (no hostname)   — receives routed tool/prompt requests
#       → authz AuthPolicy: JWT validation + CEL over x-mcp-* headers
#         checks resource_access.<server>.roles for tool:<name> entry
#       → 403 on insufficient permissions
#
#   Per-server HTTPRoutes attach to the mcps listener, not mcp.
#   x-mcp-servername format: <namespace>/<mcpserverregistration_name>
#   Keycloak client IDs must match this format exactly.
#
# Prerequisites:
#   - 01-deploy-keycloak.sh completed (realm imported with namespaced clients
#     and tool:-prefixed client roles — see realm-export.json)
#   - 06-deploy-mcp-servers.sh completed with MCP_GATEWAY_ENABLED=true
#   - 20-configure-mcp-gateway.sh completed
#   - Kuadrant operator installed on the cluster
#
# What this script does:
#   1. Patches Gateway: reconfigures listeners for the two-listener pattern
#   2. Patches MCPGatewayExtension with oauthProtectedResource (OAuth discovery)
#   3. Creates the mcp-gateway-authn-ssl EnvoyFilter (Authorino needs TLS)
#   4. Cleans up old single-listener AuthPolicy (if exists)
#   5. Re-deploys mcp-servers with mcpGateway.auth.enabled=true
#      (creates authn + authz AuthPolicy CRs, moves per-server routes to mcps)
#   6. Waits for AuthPolicy status
#   7. Prints verification commands
#
# Safe to re-run (idempotent).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${MCP_GATEWAY_NAME:=mcp-gateway}"
: "${MCP_GATEWAY_NAMESPACE:=openshift-ingress}"
: "${MCP_GATEWAY_REG_NAMESPACE:=mcp-gateway-system}"
: "${MCP_GATEWAY_EXT_NAME:=mcp-gateway-ext}"
: "${MCP_GATEWAY_EXT_NAMESPACE:=${MCP_GATEWAY_REG_NAMESPACE}}"
: "${MCP_GATEWAY_PUBLIC_HOST:=mcp.${CLUSTER_APPS_DOMAIN}}"

# ---------------------------------------------------------------------------
# Step 1: Patch Gateway for two-listener pattern
#
# Current state:
#   mcps  port 443  HTTPS  hostname=<public>   (0 routes)
#   mcp   port 8080 HTTP   no hostname          (all routes)
#
# Target state:
#   mcp   port 8080 HTTP   hostname=<public>    (mcp-gateway-route only)
#   mcps  port 8080 HTTP   no hostname           (per-server routes)
#
# Both listeners on port 8080 so ext_proc route-cache-clear works.
# ---------------------------------------------------------------------------
echo "=== Step 1: Patch Gateway for two-listener pattern ==="

# Replace both listeners in one patch
oc patch gateway "$MCP_GATEWAY_NAME" -n "$MCP_GATEWAY_NAMESPACE" --type json -p "$(cat <<EOF
[
  {
    "op": "replace",
    "path": "/spec/listeners",
    "value": [
      {
        "name": "mcp",
        "hostname": "${MCP_GATEWAY_PUBLIC_HOST}",
        "port": 8080,
        "protocol": "HTTP",
        "allowedRoutes": {
          "namespaces": {
            "from": "Selector",
            "selector": {
              "matchExpressions": [
                {
                  "key": "kubernetes.io/metadata.name",
                  "operator": "In",
                  "values": ["${MCP_GATEWAY_NAMESPACE}", "${MCP_GATEWAY_REG_NAMESPACE}"]
                }
              ]
            }
          }
        }
      },
      {
        "name": "mcps",
        "port": 8080,
        "protocol": "HTTP",
        "allowedRoutes": {
          "namespaces": {
            "from": "Selector",
            "selector": {
              "matchExpressions": [
                {
                  "key": "kubernetes.io/metadata.name",
                  "operator": "In",
                  "values": ["${MCP_GATEWAY_NAMESPACE}", "${MCP_GATEWAY_REG_NAMESPACE}"]
                }
              ]
            }
          }
        }
      }
    ]
  }
]
EOF
)"
echo "  ✓ Gateway patched: mcp (hostname=${MCP_GATEWAY_PUBLIC_HOST}) + mcps (catch-all), both port 8080"

# Wait for Gateway to reconcile
echo "  Waiting for Gateway to reconcile..."
sleep 5
oc get gateway "$MCP_GATEWAY_NAME" -n "$MCP_GATEWAY_NAMESPACE" \
  -o jsonpath='{range .status.listeners[*]}  {.name}: attachedRoutes={.attachedRoutes}{"\n"}{end}'

# ---------------------------------------------------------------------------
# Step 2: Patch MCPGatewayExtension with oauthProtectedResource
#
# The broker serves /.well-known/oauth-protected-resource natively when
# oauthProtectedResource is set. This tells MCP clients (Inspector, SDKs)
# which authorization server protects the gateway — they follow the
# authorization_servers array to Keycloak's own .well-known endpoint.
# ---------------------------------------------------------------------------
echo ""
echo "=== Step 2: Patch MCPGatewayExtension with oauthProtectedResource ==="

KEYCLOAK_ISSUER="https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}"

oc patch mcpgatewayextension "$MCP_GATEWAY_EXT_NAME" -n "$MCP_GATEWAY_EXT_NAMESPACE" \
  --type='merge' \
  -p "{\"spec\":{\"oauthProtectedResource\":{\"authorizationServers\":[\"${KEYCLOAK_ISSUER}\"]}}}"
echo "  ✓ oauthProtectedResource.authorizationServers set to [\"${KEYCLOAK_ISSUER}\"]"

# ---------------------------------------------------------------------------
# Step 3: Create mcp-gateway-authn-ssl EnvoyFilter
#
# Authorino serves its gRPC authorization endpoint with TLS (using a cert
# signed by the OpenShift service CA). The kuadrant-auth-<gateway> EnvoyFilter
# that Kuadrant creates adds the cluster, but omits the TLS transport socket.
# Without it, Envoy connects to Authorino in plaintext and every gRPC auth
# check fails with "gRPC status code is not OK" → 500 to the client.
# ---------------------------------------------------------------------------
echo ""
echo "=== Step 3: Create mcp-gateway-authn-ssl EnvoyFilter ==="

cat > /tmp/mcp-gateway-authn-ssl.yaml <<'ENVOYFILTER'
apiVersion: networking.istio.io/v1alpha3
kind: EnvoyFilter
metadata:
  name: mcp-gateway-authn-ssl
  namespace: openshift-ingress
spec:
  configPatches:
  - applyTo: CLUSTER
    match:
      cluster:
        service: authorino-authorino-authorization.kuadrant-system.svc.cluster.local
    patch:
      operation: ADD
      value:
        connect_timeout: 1s
        http2_protocol_options: {}
        lb_policy: ROUND_ROBIN
        load_assignment:
          cluster_name: kuadrant-auth-service
          endpoints:
          - lb_endpoints:
            - endpoint:
                address:
                  socket_address:
                    address: authorino-authorino-authorization.kuadrant-system.svc.cluster.local
                    port_value: 50051
        name: kuadrant-auth-service
        transport_socket:
          name: envoy.transport_sockets.tls
          typed_config:
            "@type": type.googleapis.com/envoy.extensions.transport_sockets.tls.v3.UpstreamTlsContext
            common_tls_context:
              validation_context:
                trusted_ca:
                  filename: /var/run/secrets/kubernetes.io/serviceaccount/service-ca.crt
        type: STRICT_DNS
  priority: -1
  targetRefs:
  - group: gateway.networking.k8s.io
    kind: Gateway
    name: mcp-gateway
ENVOYFILTER

oc apply -f /tmp/mcp-gateway-authn-ssl.yaml
echo "  ✓ mcp-gateway-authn-ssl EnvoyFilter created/updated"

# ---------------------------------------------------------------------------
# Step 4: Delete old single-listener AuthPolicy (if exists)
# ---------------------------------------------------------------------------
echo ""
echo "=== Step 4: Clean up old AuthPolicy ==="
if oc get authpolicy mcp-gateway-auth -n "$MCP_GATEWAY_REG_NAMESPACE" &>/dev/null; then
  oc delete authpolicy mcp-gateway-auth -n "$MCP_GATEWAY_REG_NAMESPACE"
  echo "  ✓ Deleted old mcp-gateway-auth AuthPolicy from ${MCP_GATEWAY_REG_NAMESPACE}"
else
  echo "  (no old mcp-gateway-auth to clean up)"
fi

# ---------------------------------------------------------------------------
# Step 5: Re-deploy mcp-servers with auth enabled
#
# This creates:
#   - authn AuthPolicy (mcp-gateway-authn) in gateway namespace
#   - authz AuthPolicy (mcp-gateway-authz) in gateway namespace
#   - Moves per-server HTTPRoutes from sectionName:mcp to sectionName:mcps
# ---------------------------------------------------------------------------
echo ""
echo "=== Step 5: Re-deploy mcp-servers with mcpGateway.auth.enabled=true ==="

: "${OPENAI_API_KEY:?set OPENAI_API_KEY in .env}"
: "${OPENAI_BASE_URL:?set OPENAI_BASE_URL in .env}"
: "${OPENAI_MODEL:?set OPENAI_MODEL in .env}"
: "${MCP_GATEWAY_SECTION:=mcp}"
: "${MCP_GATEWAY_INTERNAL_DOMAIN:=mcp.local}"
: "${MCP_GATEWAY_PORT:=8010}"

HELM_SET_ARGS=(
  --set "keycloak.issuer=https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}"
  --set "newsGenerator.openaiApiKey=${OPENAI_API_KEY}"
  --set "newsGenerator.openaiBaseUrl=${OPENAI_BASE_URL}"
  --set "newsGenerator.openaiModel=${OPENAI_MODEL}"
  --set "mcpGateway.enabled=true"
  --set "mcpGateway.gatewayName=${MCP_GATEWAY_NAME}"
  --set "mcpGateway.gatewayNamespace=${MCP_GATEWAY_NAMESPACE}"
  --set "mcpGateway.sectionName=${MCP_GATEWAY_SECTION}"
  --set "mcpGateway.registrationNamespace=${MCP_GATEWAY_REG_NAMESPACE}"
  --set "mcpGateway.internalDomain=${MCP_GATEWAY_INTERNAL_DOMAIN}"
  --set "mcpGateway.gatewayPort=${MCP_GATEWAY_PORT}"
  --set "mcpGateway.publicHost=${MCP_GATEWAY_PUBLIC_HOST}"
  --set "mcpGateway.auth.enabled=true"
  --set "mcpGateway.auth.authzSectionName=mcps"
)

if oc -n "$OPENSHELL_NAMESPACE" get configmap openshell-oidc-ca &>/dev/null; then
  HELM_SET_ARGS+=(--set "newsGenerator.caConfigMapName=openshell-oidc-ca")
fi

helm upgrade --install mcp-servers "$DEMO_DIR/mcp-servers" \
  --namespace "$OPENSHELL_NAMESPACE" \
  "${HELM_SET_ARGS[@]}"

# ---------------------------------------------------------------------------
# Step 6: Wait for AuthPolicy status
# ---------------------------------------------------------------------------
echo ""
echo "=== Step 6: Wait for AuthPolicy status ==="
echo "Waiting for AuthPolicies to reconcile..."
sleep 15

for AP in mcp-gateway-authn mcp-gateway-authz; do
  STATUS=$(oc get authpolicy "$AP" -n "$MCP_GATEWAY_NAMESPACE" \
    -o jsonpath='{range .status.conditions[*]}{.type}={.status} {end}' 2>/dev/null || echo "NotFound")
  echo "  ${AP}: ${STATUS}"
done

# Check per-server routes moved to mcps
echo ""
echo "  Listener route counts:"
oc get gateway "$MCP_GATEWAY_NAME" -n "$MCP_GATEWAY_NAMESPACE" \
  -o jsonpath='{range .status.listeners[*]}  {.name}: attachedRoutes={.attachedRoutes}{"\n"}{end}'

# ---------------------------------------------------------------------------
# Step 7: Print verification commands
# ---------------------------------------------------------------------------
echo ""
echo "=== Done ==="
echo ""
echo "Verify authentication (401) and authorization (403):"
echo ""
echo "# 1. Unauthenticated request → should return 401:"
echo "curl -sk https://${MCP_GATEWAY_PUBLIC_HOST}/mcp \\"
echo "  -H 'Content-Type: application/json' \\"
echo "  -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1.0\"}}}'"
echo ""
echo "# 2. Get alice's token (alice has banker + compatibility-user roles):"
echo "ALICE_TOKEN=\$(curl -sk -X POST 'https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}/protocol/openid-connect/token' \\"
echo "  -d 'grant_type=password&client_id=openshell-cli&username=alice&password=alice' | jq -r .access_token)"
echo ""
echo "# Inspect alice's resource_access claim:"
echo "echo \"\$ALICE_TOKEN\" | cut -d. -f2 | base64 -d 2>/dev/null | jq '.resource_access'"
echo ""
echo "# Initialize session:"
echo "SESSION=\$(curl -sk -D- 'https://${MCP_GATEWAY_PUBLIC_HOST}/mcp' \\"
echo "  -H \"Authorization: Bearer \$ALICE_TOKEN\" \\"
echo "  -H 'Content-Type: application/json' \\"
echo "  -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1.0\"}}}' \\"
echo "  | grep -i mcp-session-id | awk '{print \$2}' | tr -d '\\r')"
echo ""
echo "# 3. Alice calls mcp-portfolio tool → 200 (has tool:list_my_clients via banker):"
echo "curl -sk 'https://${MCP_GATEWAY_PUBLIC_HOST}/mcp' \\"
echo "  -H \"Authorization: Bearer \$ALICE_TOKEN\" \\"
echo "  -H \"mcp-session-id: \$SESSION\" \\"
echo "  -H 'Content-Type: application/json' \\"
echo "  -d '{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"mcp_portfolio_list_my_clients\",\"arguments\":{}}}'"
echo ""
echo "# 4. Bob calls mcp-compatibility tool → 403 (no compatibility-user role):"
echo "BOB_TOKEN=\$(curl -sk -X POST 'https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}/protocol/openid-connect/token' \\"
echo "  -d 'grant_type=password&client_id=openshell-cli&username=bob&password=bob' | jq -r .access_token)"
echo ""
echo "BOB_SESSION=\$(curl -sk -D- 'https://${MCP_GATEWAY_PUBLIC_HOST}/mcp' \\"
echo "  -H \"Authorization: Bearer \$BOB_TOKEN\" \\"
echo "  -H 'Content-Type: application/json' \\"
echo "  -d '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"1.0\"}}}' \\"
echo "  | grep -i mcp-session-id | awk '{print \$2}' | tr -d '\\r')"
echo ""
echo "curl -sk 'https://${MCP_GATEWAY_PUBLIC_HOST}/mcp' \\"
echo "  -H \"Authorization: Bearer \$BOB_TOKEN\" \\"
echo "  -H \"mcp-session-id: \$BOB_SESSION\" \\"
echo "  -H 'Content-Type: application/json' \\"
echo "  -d '{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"mcp_compatibility_calc_tax\",\"arguments\":{\"income\":100000}}}'"
echo ""
echo "# Expected: 401 for unauthenticated, 200 for authorized, 403 for unauthorized."
