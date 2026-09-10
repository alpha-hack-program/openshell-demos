# audit-dashboard — a live risk/heartbeat graph for audited sandboxes

**Meridian Inc.**'s live risk/heartbeat dashboard: a small standing service
that draws `user → sandbox → MCP server` and colors each sandbox by
[`util/session-auditor`](../session-auditor/)'s compliance-risk score
(green → red) and liveness (dimmed once its heartbeat goes stale, never
removed — see below) — built for Scene 7 ("Watching the audit trail live")
in [`demos/keycloak-oidc/README.md`](../../demos/keycloak-oidc/README.md).

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
  own bound ServiceAccount token for auth. The RBAC
  (`cluster-monitoring-view` ClusterRole binding, see
  `demos/keycloak-oidc/audit-dashboard/templates/clusterrolebinding.yaml`)
  works with no extra grant needed. Trusting only
  `/var/run/secrets/kubernetes.io/serviceaccount/ca.crt` (the Kubernetes
  API server's CA) fails verification against Thanos-querier's serving
  certificate, which OpenShift signs with a separate internal service-ca
  instead — its bundle is auto-projected into every pod as a sibling
  `service-ca.crt`, undocumented but present by default. `build_http_client`
  now trusts both.
- Every `refreshIntervalSecs` (default `5`), the backend re-runs three
  PromQL queries scoped to its own namespace
  (`agent_session_started`/`agent_turn_heartbeat`/
  `session_compliance_risk_score{namespace="..."}`) and **merges** the
  results into the existing in-memory graph, and serves it as JSON from
  `GET /api/graph`. A sandbox (and its MCP-server edges) is added the
  first time it's observed and never removed — not even once Prometheus's
  own instant-query lookback window passes and it stops showing up in
  query results. Liveness is conveyed entirely by the frontend dimming
  stale nodes, never by dropping them; an earlier "rebuild from scratch
  every tick" version made sandboxes vanish outright once Prometheus
  stopped returning a sample, which read as data loss rather than "this
  one's offline." If `STATE_FILE_PATH` is set, the sandbox list is also
  written to that path (typically a mounted PVC — see
  `demos/keycloak-oidc/audit-dashboard/values.yaml`'s `persistence`
  block) after every refresh and reloaded at startup, so this "seen once,
  shown forever" graph survives a pod restart too.
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
  low. Dimming, not disappearing, is deliberate: once a sandbox has been
  observed it stays on the graph for the life of the backend (see "Why
  it's built this way" above), so a quiet sandbox reads as "went offline,"
  never as "was never there."
- **A short pulse ring** flashes around a sandbox node in its own risk
  color whenever a fresh heartbeat sample lands for it — a quick visual
  "this one's alive right now," distinct from the steady-state fill color.
- **Edges to MCP-server nodes** come from the `mcp_servers` attribute on
  `session_compliance_risk_score` (comma-joined server short-names).
  A sandbox that has never completed a `Stop` hook (e.g. no classification credential
  configured) shows no MCP edges yet, even if it's live.

## Configuration

| Env var | Default | Description |
|---|---|---|
| `OPENSHELL_NAMESPACE` | *(required)* | Every PromQL query is scoped to this namespace — the dashboard only ever shows its own demo's sandboxes. |
| `THANOS_QUERIER_URL` | `https://thanos-querier.openshift-monitoring.svc:9091` | In-cluster Thanos-querier base URL |
| `REFRESH_INTERVAL_SECS` | `5` | How often to re-poll and merge new samples into the graph. |
| `HEARTBEAT_STALE_SECS` | `90` | How long since the last heartbeat before a sandbox renders dimmed/offline. |
| `STATE_FILE_PATH` | *(unset)* | Optional path (typically a mounted PVC) to persist the sandbox list to after every refresh and reload at startup. In-memory-only if unset. |
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
