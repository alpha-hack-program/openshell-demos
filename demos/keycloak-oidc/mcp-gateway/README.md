# mcp-gateway Helm chart

Cluster-scoped RHCL (Kuadrant) MCP Gateway resources: `GatewayClass`,
`Gateway`, `MCPGatewayExtension`, a ConfigMap forcing the gateway's Service
to `ClusterIP`, and (when `auth.enabled`) the Authorino-TLS `EnvoyFilter`
workaround. This is everything [`../mcp-servers`](../mcp-servers/)'s own
`mcpGateway.enabled`/`mcpGateway.auth.enabled` attaches to when routing
through the gateway instead of direct sandbox-to-server access.

Split out of `mcp-servers` because these resources are shared
infrastructure, not owned by any one demo's namespace/release: `Gateway`
and `MCPGatewayExtension` live in `openshift-ingress`/`mcp-gateway-system`,
not `OPENSHELL_NAMESPACE` — `helm uninstall mcp-servers` should never take
them down. Confirmed live against sandbox268: this chart reproduces
exactly what scripts/20 and scripts/21 previously built by hand-patching
resources that were assumed to already exist.

## Prerequisites

- The `rhcl-operator` CSV (Red Hat Connectivity Link, Kagenti/Kuadrant MCP
  gateway, v0.7.1+) installed, plus a `Kuadrant` CR applied to instantiate
  its Authorino/Limitador operands — needed either way for the
  `GatewayClass`/`Gateway` below, and (if `auth.enabled`, default `true`)
  for `mcp-servers`' own `AuthPolicy` CRs to reconcile. One Subscription:
  `rhcl-operator` pulls in `authorino-operator`/`limitador-operator` as OLM
  dependencies — there's no separate "Kuadrant operator" to install. This
  chart only creates CRs their CRDs define, never the operators or the
  `Kuadrant` CR itself. See the main demo README's
  [Installing RHCL](../README.md#installing-rhcl) for the install steps
  and gotchas.

## Adopting already-existing resources

If the `Gateway`/`MCPGatewayExtension`/etc. already exist on your cluster
(e.g. hand-created by the old `scripts/20`/`scripts/21`, before this chart
existed), `helm install` will refuse them with `"... exists and cannot be
imported into the current release"` — Helm only manages resources it
created, identified by its own annotations/label. Adopt them first (once,
per resource, substituting `RELEASE_NS` for whatever `--namespace` you
pass to `helm install` below):

```bash
RELEASE_NS=openshell   # or whatever namespace you run `helm install` under
for res in "gatewayclass/mcp-gateway-class" \
           "configmap/mcp-gateway-override-config -n openshift-ingress" \
           "gateway/mcp-gateway -n openshift-ingress" \
           "mcpgatewayextension/mcp-gateway-ext -n mcp-gateway-system" \
           "envoyfilter/mcp-gateway-authn-ssl -n openshift-ingress"; do
  oc annotate $res meta.helm.sh/release-name=mcp-gateway meta.helm.sh/release-namespace="$RELEASE_NS" --overwrite
  oc label $res app.kubernetes.io/managed-by=Helm --overwrite
done
```

Skip the `envoyfilter` line if you're installing with `auth.enabled=false`
(this chart won't create/manage it in that mode). Nothing to adopt on a
cluster where these resources don't exist yet — just `helm install`
directly.

## Install

Once per cluster, before any demo enables `mcpGateway` in `mcp-servers`:

```bash
helm upgrade --install mcp-gateway ./mcp-gateway \
  --set gateway.publicHost="mcp.${CLUSTER_APPS_DOMAIN}" \
  --set keycloak.issuer="https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}"
```

Idempotent — safe to re-run, and safe to leave installed while any demo's
`mcp-servers` release is installed/upgraded/uninstalled independently.

## Then, in mcp-servers

```bash
helm upgrade --install mcp-servers ../mcp-servers \
  --namespace "$OPENSHELL_NAMESPACE" \
  --set mcpGateway.enabled=true \
  --set mcpGateway.auth.enabled=true \
  --set mcpGateway.publicHost="mcp.${CLUSTER_APPS_DOMAIN}" \
  ...
```

`mcp-servers`' `mcpGateway.{gatewayName,gatewayNamespace,publicHost}` and
`mcpGateway.auth.authzSectionName` must match this chart's
`gateway.{name,namespace,publicHost}` and its fixed listener names
(`mcp`/`mcps`) — `mcpGateway.registrationNamespace` must match this
chart's `extension.namespace`.

## Values reference

| Key | Default | Description |
|---|---|---|
| `gateway.name` | `mcp-gateway` | `Gateway` CR name. |
| `gateway.namespace` | `openshift-ingress` | `Gateway` CR namespace. |
| `gateway.className` | `mcp-gateway-class` | `GatewayClass` this chart also creates. |
| `gateway.controllerName` | `openshift.io/gateway-controller/v1` | OpenShift's built-in Gateway API controller — not RHCL-specific. |
| `gateway.port` | `8080` | Port shared by both listeners (required for ext_proc's route-cache-clear to reach `mcps`). |
| `gateway.publicHost` | `""` | Public hostname — required, e.g. `mcp.apps.<cluster-domain>`. |
| `gateway.allowedNamespaces` | `[openshift-ingress, mcp-gateway-system]` | Namespaces allowed to attach HTTPRoutes/AuthPolicies. |
| `extension.name` / `extension.namespace` | `mcp-gateway-ext` / `mcp-gateway-system` | `MCPGatewayExtension` CR. |
| `keycloak.issuer` | placeholder | `oauthProtectedResource.authorizationServers` (auth mode only). |
| `auth.enabled` | `true` | Two-listener authn+authz pattern vs. a single unauthenticated listener — see below. |
| `auth.authorinoNamespace` / `auth.authorinoService` | `kuadrant-system` / `authorino-authorino-authorization` | Target of the Authorino-TLS `EnvoyFilter`. |

## Two listeners, one port

`auth.enabled: true` creates `mcp` (hostname set, external entry,
authn-only via `mcp-servers`' `mcp-gateway-authn` AuthPolicy) and `mcps`
(no hostname, internal post-routing, authn+authz via `mcp-gateway-authz`)
— RHCL 1.4's documented two-listener pattern. Both stay on the same port
so the gateway's ext_proc can rewrite `:authority` and re-evaluate routing
across listeners after parsing the JSON-RPC body.

`auth.enabled: false` creates only `mcp`, without a hostname —
unauthenticated at the gateway layer entirely. Each server's own Envoy
sidecar still enforces JWT+RBAC on its direct port either way (see
`../mcp-servers/README.md`'s "Dual-port Envoy sidecar") — this only
controls whether the *gateway* adds a second, independent check.

## Verifying

```bash
oc get gatewayclass mcp-gateway-class
oc get gateway mcp-gateway -n openshift-ingress
oc get configmap mcp-gateway-override-config -n openshift-ingress
oc get mcpgatewayextension mcp-gateway-ext -n mcp-gateway-system
oc get envoyfilter mcp-gateway-authn-ssl -n openshift-ingress   # auth.enabled only
```
