# audit-dashboard — a live risk/heartbeat graph for audited sandboxes

A small standing service that draws `user → sandbox → MCP server` and
colors each sandbox by [`util/session-auditor`](../session-auditor/)'s
compliance-risk score (green → red) and liveness (dimmed once its
heartbeat goes stale) — built for Scene 7 ("Watching the audit trail
live") in
[`demos/keycloak-oidc/README.md`](../../demos/keycloak-oidc/README.md).

## Why it's built this way

Everything the dashboard shows is **inferred from Prometheus**, not from a
static config of who's provisioned what — the whole point is that it
reflects whatever `session-auditor` has actually observed, live, rather
than a hand-maintained topology that can drift from reality. That means
the graph only ever shows sandboxes that have pushed at least one
`agent_session_started`/`agent_turn_heartbeat` sample; nothing is drawn
speculatively.

- **Backend: Rust + axum**, same shape as
  [`util/onboarding-web`](../onboarding-web/) (a standing in-cluster
  service with its own Route/Service/ServiceAccount) — but unlike
  `onboarding-web`, this service never calls the `openshell` CLI or the
  gateway at all. Its only outbound call is to
  `openshift-user-workload-monitoring`'s Thanos-querier, using this pod's
  own bound ServiceAccount token for auth. **Confirmed live** (2026-09-08,
  sandbox268) — the first workload in this repo to call `thanos-querier`
  over the network at all (the only prior precedent is an admin's
  `oc exec` straight into the Prometheus pod, see
  `demos/keycloak-oidc/audit-collector/README.md`). The RBAC
  (`cluster-monitoring-view` ClusterRole binding, see
  `demos/keycloak-oidc/audit-dashboard/templates/clusterrolebinding.yaml`)
  worked on the first try with no extra grant needed. What actually broke
  on the first run was TLS, not RBAC: trusting only
  `/var/run/secrets/kubernetes.io/serviceaccount/ca.crt` (the Kubernetes
  API server's CA) fails verification against Thanos-querier's serving
  certificate, which OpenShift signs with a separate internal service-ca
  instead — its bundle is auto-projected into every pod as a sibling
  `service-ca.crt`, undocumented but present by default. `build_http_client`
  now trusts both.
- Every `refreshIntervalSecs` (default `5`), the backend re-runs three
  PromQL queries scoped to its own namespace
  (`agent_session_started`/`agent_turn_heartbeat`/
  `session_compliance_risk_score{namespace="..."}`), rebuilds an in-memory
  graph from scratch (simpler than incrementally patching state from a
  diff, and cheap at this demo's scale), and serves it as JSON from
  `GET /api/graph`.
- **Frontend: Preact**, bundled with `esbuild` (single dev dependency, no
  bundler ceremony) into a static `dist/bundle.js` + `index.html`, served
  directly by the same axum process via `tower_http::services::ServeDir` —
  no separate frontend server, no SPA routing (it's one page).
- **The dashboard never talks to `session-auditor` directly** — the two
  are connected only through Prometheus. This means the dashboard has no
  way to distinguish "no data because nothing's provisioned yet" from
  "no data because the collector/ServiceMonitor pipeline is broken" —
  check `demos/keycloak-oidc/audit-collector/README.md`'s own verification
  steps first if the graph stays empty.

## What the graph shows

- **Sandbox node fill color** interpolates green → amber → red for
  `session_compliance_risk_score` `0` → `3`. Hovering a node shows
  `risk_level` (`none`/`self_refused`/`blocked_attempt`/
  `complied_or_fabricated` — see `util/session-auditor/prompt.txt`) as the
  "why," not the free-text `evidence` field — that's deliberately never
  pushed as a Prometheus label (unbounded cardinality), so the dashboard
  can't show it. A sandbox with no risk verdict yet at all renders a
  neutral dark green, not grey — "hasn't misbehaved" and "hasn't been
  checked" look almost the same on purpose; only a stale heartbeat (below)
  should read as "unknown state."
- **Dimmed/greyed nodes** are sandboxes with no `agent_session_started`/
  `agent_turn_heartbeat` sample within `heartbeatStaleSecs` (default `90`)
  — this is a liveness signal independent of risk color, so an offline
  sandbox stays visibly distinct even if its last known risk score was
  low.
- **Edges to MCP-server nodes** come from the `mcp_servers` attribute on
  `session_compliance_risk_score` (comma-joined server short-names).
  Confirmed live for Claude Code (2026-09-08): a benign "biggest client"
  turn correctly showed edges to `portfolio`/`market-news`, the two
  servers actually called. Still `[VERIFY]` for Codex — see
  `util/session-auditor/README.md`'s own note on this. A sandbox that has
  never completed a `Stop` hook (e.g. no classification credential
  configured) shows no MCP edges yet, even if it's live.

## Configuration

| Env var | Default | Description |
|---|---|---|
| `OPENSHELL_NAMESPACE` | *(required)* | Every PromQL query is scoped to this namespace — the dashboard only ever shows its own demo's sandboxes. |
| `THANOS_QUERIER_URL` | `https://thanos-querier.openshift-monitoring.svc:9091` | In-cluster Thanos-querier base URL — confirmed live on sandbox268. |
| `REFRESH_INTERVAL_SECS` | `5` | How often to re-poll and rebuild the graph. |
| `HEARTBEAT_STALE_SECS` | `90` | How long since the last heartbeat before a sandbox renders dimmed/offline. |
| `PORT` | `8080` | HTTP port the service listens on. |

## Development

```bash
make help       # list all targets
make frontend   # npm ci + npm run build (frontend/dist)
make build      # debug build (backend only)
make check      # fmt-check + clippy
make run ARGS="--namespace my-namespace"   # builds the frontend, symlinks dist/, runs the backend
```

No automated test suite yet, matching `onboarding-web`'s own stated
convention — verified by deploying the chart
(`demos/keycloak-oidc/audit-dashboard/`) against a live cluster and
confirming `/api/graph` returns real data once a `session-auditor` push
has happened.

## Deployment

See [`demos/keycloak-oidc/audit-dashboard/`](../../demos/keycloak-oidc/audit-dashboard/)
for the Helm chart, and [`Containerfile`](Containerfile) for the
three-stage image build (Preact frontend, Rust backend, minimal runtime).

## Releasing

Same `cargo-release` flow as `onboarding-web`/`session-auditor`:

```bash
cargo install cargo-release   # if not already installed
make release-patch   # or release-minor / release-major
```
