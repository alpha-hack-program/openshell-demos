# audit-dashboard

Deploys [`util/audit-dashboard`](../../../util/audit-dashboard/) — a live
`user → sandbox → MCP server` graph, colored by
[`session-auditor`](../../../util/session-auditor/)'s risk/heartbeat
metrics, queried directly from
`openshift-user-workload-monitoring`'s Thanos-querier. See
[`util/audit-dashboard/README.md`](../../../util/audit-dashboard/README.md)
for the full design, and
[`../docs/prometheus-scraping.md`](../docs/prometheus-scraping.md) for the
metrics this reads.

## Prerequisites

- [`../audit-collector/`](../audit-collector/) already deployed (this
  chart reads what that one collects — deploy it first).
- At least one sandbox provisioned via
  `../scripts/16-provision-audited-sandbox.sh` and pushing metrics, or the
  graph will simply be empty (not an error — see "Verifying it works").
- The built-in `cluster-monitoring-view` ClusterRole must exist —
  **confirmed live (2026-09-08, sandbox268)** that binding it to this
  chart's own `ServiceAccount` is sufficient for Thanos-querier to answer
  namespace-scoped queries; no additional RBAC needed. This was the first
  time this repo had a workload call `thanos-querier` in-cluster at all
  (the only prior precedent, `../audit-collector/README.md`, is an
  admin's `oc exec` straight into the Prometheus pod) — it works, but the
  real blocker turned out to be TLS, not RBAC (see below).

## Install

```bash
helm upgrade --install audit-dashboard demos/keycloak-oidc/audit-dashboard \
  --namespace "$OPENSHELL_NAMESPACE"
```

Override `route.host` if you need a stable hostname (optional — this app
has no OAuth callback to pre-register anywhere, unlike `onboarding-web`,
so an OpenShift-auto-generated one is fine for a demo):

```bash
helm upgrade --install audit-dashboard demos/keycloak-oidc/audit-dashboard \
  --namespace "$OPENSHELL_NAMESPACE" \
  --set route.host="$AUDIT_DASHBOARD_ROUTE_HOST"
```

## Verifying it works

```bash
oc get route audit-dashboard -n "$OPENSHELL_NAMESPACE" -o jsonpath='{.spec.host}'
```

Open that host in a browser. An empty graph with no errors means no
audited sandbox has pushed a metric yet — provision one via
`../scripts/16-provision-audited-sandbox.sh` and run a turn in it, then
wait up to `refreshIntervalSecs` (default `5s`) for the next poll.

If the page itself fails to load, check the pod's own logs
(`oc logs -n "$OPENSHELL_NAMESPACE" deploy/audit-dashboard`). A
`thanos-querier returned 403` points at the ClusterRoleBinding being
missing/wrong. A `request to thanos-querier failed: ... certificate`
error (confirmed live as the actual first-run failure mode, not RBAC)
means the running image predates the `service-ca.crt` fix — rebuild
against a current `util/audit-dashboard` checkout and redeploy.

## Values reference

| Value | Default | Description |
|---|---|---|
| `image.{repository,tag}` | `quay.io/atarazana/audit-dashboard:0.1.0` | Image to deploy. |
| `route.host` | `""` | Optional fixed hostname; leave empty for an OpenShift-auto-generated one. |
| `prometheus.thanosQuerierUrl` | `https://thanos-querier.openshift-monitoring.svc:9091` | In-cluster Thanos-querier base URL — confirmed live on sandbox268. |
| `refreshIntervalSecs` | `5` | How often the backend re-polls Prometheus. |
| `heartbeatStaleSecs` | `90` | How long since the last heartbeat before a sandbox renders dimmed/offline. |

Confirmed live end to end (2026-09-08, sandbox268): after a benign turn
in an audited sandbox, its node showed `risk_level: none`; after
repeating [Scene 4a](../README.md#scene-4a--bob-overreaches)'s forcing
prompt in the same sandbox, it flipped to `risk_level: blocked_attempt`
(`score: 2`) with `mcp_servers` correctly attributed — visible both via
`GET /api/graph` and through the real HTTPS Route.

## Known limitations

- **Unauthenticated.** Anyone who can reach the Route sees every audited
  sandbox's risk/heartbeat state in this namespace. Acceptable for a demo
  audience; don't reuse this chart as-is beyond that without adding auth
  in front of it.
- Single replica, no persistence — a pod restart just means an empty graph
  until the next successful poll re-populates it from Prometheus (which
  itself still has the history; nothing is lost, just not shown until the
  next tick).
- Shows only what `session-auditor` has pushed recently (bounded by
  Prometheus's own ~5-minute staleness window on top of
  `heartbeatStaleSecs`) — it is a live view, not a historical audit log.
