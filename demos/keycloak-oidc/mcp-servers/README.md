# mcp-servers Helm chart

Deploys the keycloak-oidc demo's MCP servers (mcp-compatibility,
mcp-portfolio, mcp-crm-calendar, mcp-market-news, mcp-kyc-compliance) with
an Envoy sidecar that enforces Keycloak JWT authentication and per-server
RBAC via `realm_access.roles`.

## Dual-port Envoy sidecar

Each server pod runs an Envoy sidecar with two listeners:

| Port | Listener | Auth | Used by |
|------|----------|------|---------|
| 8000 | `listener_0` | JWT signature + RBAC role check | Direct user access (e.g. `curl -H "Authorization: Bearer $TOKEN"`) |
| 8010 | `gateway_listener` | None | RHCL MCP gateway broker (ext_proc routes `tools/call` here) |

Port 8010 skips auth because the RHCL gateway enforces authentication
upstream — the broker already validated the caller before routing the
`tools/call` request to the backend.

## RHCL MCP Gateway integration

When `mcpGateway.enabled: true`, the chart creates per-server HTTPRoute and
MCPServerRegistration resources in the gateway's registration namespace.

### How internal hostname routing works

The RHCL gateway's ext_proc (MCP router) routes `tools/call` requests by
rewriting the `:authority` (Host) header:

1. Client sends `tools/call` with `{"name": "mcp_compatibility_calc_tax", ...}`
   to the gateway's public endpoint (e.g. `mcp.apps.example.com`).
2. The broker's HTTPRoute at `/mcp` receives it first.
3. The ext_proc parses the JSON-RPC body, identifies the target server by
   matching the tool name prefix (`mcp_compatibility_`) against
   MCPServerRegistration prefixes.
4. It sets `:authority` to the backend's internal hostname
   (`mcp-compatibility.mcp.local`), strips the prefix from the tool name
   (so the backend sees `calc_tax`), and clears Envoy's route cache.
5. Envoy re-evaluates routing and matches the per-server HTTPRoute's virtual
   host, forwarding to the backend Service on port 8010.

The internal hostname (e.g. `mcp-compatibility.mcp.local`) is purely an
Envoy virtual-host label — **no DNS entry is needed**. The actual TCP
connection goes to the Kubernetes Service defined in the HTTPRoute's
`backendRefs`.

### Prerequisites

Before enabling `mcpGateway`:

1. **Install the RHCL operator** (Kagenti/Kuadrant MCP gateway, v0.7.1+).

2. **Gateway listener must NOT have a `hostname` field.** If the MCP
   listener (the one MCPGatewayExtension targets via `sectionName`) has a
   `hostname`, only HTTPRoutes matching that exact hostname are accepted —
   internal hostnames like `mcp-compatibility.mcp.local` would be rejected.
   Remove it:
   ```bash
   # Find the listener index (0-based) for your MCP listener
   oc get gateway mcp-gateway -n openshift-ingress -o jsonpath='{.spec.listeners}' | python3 -m json.tool
   # Remove the hostname field (adjust index as needed)
   oc patch gateway mcp-gateway -n openshift-ingress \
     --type='json' \
     -p='[{"op": "remove", "path": "/spec/listeners/1/hostname"}]'
   ```

3. **Set `spec.publicHost` on the MCPGatewayExtension** (required when the
   listener has no hostname):
   ```bash
   oc patch mcpgatewayextension mcp-gateway-ext -n mcp-gateway-system \
     --type='merge' \
     -p '{"spec":{"publicHost":"mcp.apps.example.com"}}'
   ```

4. **Gateway's `allowedRoutes.namespaces`** must include
   `registrationNamespace` (default: `mcp-gateway-system`).

### Configuration

```yaml
mcpGateway:
  enabled: true
  gatewayName: mcp-gateway            # Gateway CR name
  gatewayNamespace: openshift-ingress  # Gateway CR namespace
  sectionName: mcp                     # Listener name on the Gateway
  registrationNamespace: mcp-gateway-system  # Where HTTPRoute + MCPServerRegistration are created
  internalDomain: mcp.local            # Domain suffix for routing hostnames (no DNS needed)
  gatewayPort: 8010                    # Envoy sidecar no-auth port
```

### What gets created

For each server in `values.yaml`:

- **HTTPRoute** `<name>-route` in `registrationNamespace` — hostname
  `<name>.<internalDomain>`, routes to the server's Service on
  `gatewayPort`.
- **MCPServerRegistration** `<name>` in `registrationNamespace` — prefix
  `<name_with_underscores>_`, targets the HTTPRoute.

### Verifying

After `helm upgrade --install`:

```bash
# Check registrations are accepted
oc get mcpserverregistrations -n mcp-gateway-system

# Check HTTPRoutes are accepted by the gateway
oc get httproutes -n mcp-gateway-system

# List tools via the broker
curl -sk https://mcp.apps.example.com/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/list"}'

# Call a tool
curl -sk https://mcp.apps.example.com/mcp \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"mcp_compatibility_calc_tax","arguments":{"income":100000}}}'
```
