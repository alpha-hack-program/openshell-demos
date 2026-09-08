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
- **`[VERIFY]`**: the built-in `cluster-monitoring-view` ClusterRole must
  exist and actually grant Thanos-querier read access for this namespace's
  metrics — confirm against your cluster
  (`oc get clusterrole cluster-monitoring-view`) before assuming this
  chart's `ClusterRoleBinding` is sufficient. This repo has no prior
  confirmed pattern for a workload calling `thanos-querier` in-cluster —
  the only existing precedent
  (`../audit-collector/README.md`) is an admin's `oc exec` straight into
  the Prometheus pod.

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
(`oc logs -n "$OPENSHELL_NAMESPACE" deploy/audit-dashboard`) — a
`thanos-querier returned 403` there points at the `[VERIFY]` RBAC item
above, not a bug in the dashboard itself.

## Values reference

| Value | Default | Description |
|---|---|---|
| `image.{repository,tag}` | `quay.io/atarazana/audit-dashboard:0.1.0` | Image to deploy. |
| `route.host` | `""` | Optional fixed hostname; leave empty for an OpenShift-auto-generated one. |
| `prometheus.thanosQuerierUrl` | `https://thanos-querier.openshift-monitoring.svc:9091` | In-cluster Thanos-querier base URL. `[VERIFY]` on your cluster. |
| `refreshIntervalSecs` | `5` | How often the backend re-polls Prometheus. |
| `heartbeatStaleSecs` | `90` | How long since the last heartbeat before a sandbox renders dimmed/offline. |

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
