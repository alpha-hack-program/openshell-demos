# Spike: scraping a sandbox metrics endpoint with Prometheus

Status: **Experimental / proof-of-concept — not a settled, production-ready
pattern.** Validated end-to-end on a live cluster (2026-08-28, OpenShell
0.0.106, OpenShift 4.21.18 / Kubernetes 1.34.8, prometheus-operator 3.7.3).
Not merged — see recommendation at the end. All cluster state created for
this spike (sandbox, workspace, exposed service, ServiceMonitor, both the
`garak-envoy` and `prometheus-envoy` Helm releases) was torn down after
each validation pass; nothing was left running.

**Follow-up spike:** [session-auditor-spike.md](session-auditor-spike.md)
builds on this to answer a harder question — can the metric itself be
trusted, i.e. protected from tampering by the sandbox's own user, not just
reachable by Prometheus.

## Question

Can Prometheus scrape a metrics endpoint exposed from an OpenShell sandbox
on OpenShift, given that `openshell service expose` routes purely by HTTP
Host header through a single shared gateway Route (see [Sandbox service
patterns](../../../docs/sandbox-service-patterns.md))?

## Answer

**Yes, but only through an Envoy Host-rewrite proxy — not via a native
ServiceMonitor `headers` field**, because this cluster's prometheus-operator
version (3.7.3) has no `headers` field on `ServiceMonitor.spec.endpoints`
(confirmed via `oc explain servicemonitor.spec.endpoints` — the full field
list has no `headers` entry, only `params`, `authorization`, `basicAuth`,
etc.). This is the exact same class of problem
[`docs/evalhub-redteam.md`](evalhub-redteam.md) solved for Garak/EvalHub,
and the same fix applies unchanged.

## What was tested

1. Built `util/metrics-file-exporter/` — a ~60 LOC Rust/axum service
   (Rust 2024 edition, `x86_64-unknown-linux-musl` target) serving
   `GET /metrics` from a file re-read on every request (`METRICS_FILE` env
   var / `--metrics-file` flag), matching `util/agent-proxy`'s conventions.
2. Created a disposable `spike` workspace and a `metrics-spike` sandbox from
   the stock `codex` base image (Approach A — upload, no custom image; see
   [Sandbox service patterns §1](../../../docs/sandbox-service-patterns.md)).
3. Uploaded the musl binary and a sample `spike_metric_value 42` file,
   started the exporter with `nohup ... &`, and confirmed via
   `openshell service expose metrics-spike 8100 --workspace spike` +
   `curl -H "Host: spike--metrics-spike.openshell.localhost"` through the
   gateway Route that the raw HTTP path works end to end.
4. Confirmed **why** a Prometheus pod can't reach the sandbox directly,
   bypassing the gateway: per
   [Sandbox network isolation](sandbox-network-isolation.md), the process
   started by `sandbox exec`/`nohup` runs in a nested network namespace
   (its own `10.200.0.0/24`) connected to the outer pod only by a single
   veth pair to the supervisor's own proxy — it has **no presence on the
   real pod network at all**, so no `NetworkPolicy` question even applies;
   there is no pod-IP route to it from any other pod, Prometheus included.
   The gateway (with its own internal routing table keyed by
   workspace/sandbox/port) is the only way in, confirming Host-header (or
   Envoy-rewrite) routing is mandatory, not just convenient.
5. Confirmed via `oc explain servicemonitor.spec.endpoints` that this
   cluster's ServiceMonitor CRD has no `headers` field — ruling out the
   "native" path from the original test plan.
6. Deployed `demos/keycloak-oidc/garak-envoy` **unmodified** (it already
   does exactly what's needed: extract a routing key from the request path
   `/route/<host-key>/...` and rewrite the upstream Host/`:authority`,
   fronted by a real ClusterIP Service) and pointed a ServiceMonitor at its
   Service with `path: /route/spike--metrics-spike.openshell.localhost/metrics`,
   `scheme: http`, no custom headers.
7. **Confirmed end-to-end**: the target showed `health: up` in Prometheus's
   own `/api/v1/targets`, and `spike_metric_value` was queryable via
   Prometheus's own `/api/v1/query` — first at `42`, then re-verified at
   `99` after re-uploading the sample file, proving the exporter's
   no-cache re-read behavior flows all the way through to a live scrape.
8. `openshift-user-workload-monitoring` was **already enabled** on this
   cluster — no `cluster-monitoring-config` change was needed, and none was
   made.

## Working ServiceMonitor config

`demos/keycloak-oidc/prometheus-envoy/templates/servicemonitor.yaml` now
generates this automatically from `values.yaml` (see below) — this is the
raw shape it produces for reference/hand-rolled use:

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: prometheus-envoy
  namespace: keycloak-oidc-demo
spec:
  selector:
    matchLabels:
      app.kubernetes.io/name: prometheus-envoy
  endpoints:
  - port: http
    path: /route/<workspace>--<sandbox-name>.openshell.localhost/metrics
    scheme: http
    interval: 30s
    relabelings:
    - targetLabel: sandbox
      replacement: <sandbox-name>
```

**Multiple sandboxes, at will:** `values.yaml` takes a `serviceMonitor.targets`
list (`name`, `hostKey`, optional `path`) — the chart renders one
ServiceMonitor with one `endpoints[]` entry per target, so adding/removing a
sandbox from monitoring is a `helm upgrade` with an updated values list, no
per-sandbox Kubernetes object to hand-write:

```yaml
serviceMonitor:
  enabled: true
  targets:
    - name: metrics-spike
      hostKey: spike--metrics-spike.openshell.localhost
```

Every target physically scrapes through the *same* Envoy pod/port, so
Prometheus's own `instance`/`pod`/`service` target labels are identical
across all of them — each `endpoints[]` entry still gets its own `job`
label (`serviceMonitor/<ns>/prometheus-envoy/<index>`, which alone prevents
metric collisions), and the chart additionally injects an explicit
`sandbox="<name>"` label per target via `relabelings` for readable
querying. **Confirmed live** with two example targets rendered
(`helm template`) and one deployed end-to-end
(`spike_metric_value{sandbox="metrics-spike"}` queryable via Prometheus's
own API) — see [Annex: chart-driven re-verification](#annex-chart-driven-re-verification-2026-08-28)
below.

**Timing note:** after `oc apply`/`helm upgrade`, a new ServiceMonitor took
anywhere from ~30s to ~2-3 minutes to actually appear in Prometheus's
`/api/v1/status/config` and start showing as an active target across the
two validation passes in this spike — don't conclude a ServiceMonitor was
rejected just because it's missing from the target list shortly after
creation; check `oc logs deployment/prometheus-operator` for actual errors
before assuming failure.

## `garak-envoy` vs `prometheus-envoy`

The existing `demos/keycloak-oidc/garak-envoy` chart was reused
**unmodified** during validation (no functional differences needed — it's
generic path-based Host-rewrite, unaware of Garak specifically). This spike
adds `demos/keycloak-oidc/prometheus-envoy/` as an identical copy renamed
for this use case, so a demo/spike that only cares about metrics scraping
doesn't have to depend on a chart named after an unrelated red-team tool.
If both ever ship for real, consider consolidating into one
generically-named chart (e.g. `host-rewrite-envoy`) parameterized by
release name, rather than maintaining two identical copies.

## Annex: chart-driven re-verification (2026-08-28)

Follow-up pass after the initial spike, to confirm the ServiceMonitor is
actually shipped in the chart (not just a hand-written `oc apply` manifest)
and that multiple sandboxes don't collide:

1. Recreated the `spike` workspace/`metrics-spike` sandbox, re-uploaded the
   exporter + sample file, re-exposed the service — identical to the first
   pass.
2. `helm upgrade --install prometheus-envoy demos/keycloak-oidc/prometheus-envoy
   -n keycloak-oidc-demo -f <values with serviceMonitor.enabled=true and one target>`.
3. Confirmed the rendered `ServiceMonitor/prometheus-envoy` object matched
   the hand-written version exactly (same path, plus the `relabelings`
   block), and — after the timing delay noted above — that the target
   showed `health: up` with `labels.sandbox: metrics-spike` in Prometheus's
   own `/api/v1/targets`.
4. Queried `spike_metric_value` via Prometheus's `/api/v1/query`: returned
   `{"sandbox":"metrics-spike", ...} = 42` — the chart-driven path produces
   the identical, queryable result as the original hand-rolled manifest.
5. `helm template` with two example targets (different `hostKey`/`path`,
   one with a custom `path`) rendered two independent `endpoints[]` blocks
   with distinct `sandbox` labels, confirming the multi-target case is
   structurally correct (not independently deployed/scraped live — only
   the single-target case was live-verified end-to-end).
6. Full cleanup: `helm uninstall prometheus-envoy`, exposed service,
   sandbox, and `spike` workspace all deleted; confirmed no leftover
   `oc get sandbox,route,svc,servicemonitor,deployment` matches afterward.

## Open items / not tested

- Only tested against `openshift-user-workload-monitoring`'s Prometheus.
  Behavior against a fully custom (non-OpenShift) Prometheus Operator
  install wasn't checked, though the mechanism (ServiceMonitor → Service →
  Envoy → gateway) doesn't depend on anything OpenShift-specific.
- `PodMonitor` wasn't checked for a `headers` field — `ServiceMonitor` was
  the only CRD inspected, since the exporter is fronted by a Service either
  way (`garak-envoy`'s own Service).
- No load/scale testing — only one target was ever scraped live at once;
  the multi-target case was confirmed by `helm template` rendering only
  (see Annex above), not by actually running two sandboxes' scrapes
  concurrently through the same Envoy pod.
- The `garak-envoy`/`prometheus-envoy` image tag (`envoyproxy/envoy:v1.31-latest`)
  is still marked `[VERIFY]` in `values.yaml` — inherited from the original
  chart, not re-verified in this spike.

## Recommendation

**Merge or discard is the user's call — not decided here.** Findings for
that decision:

- The mechanism works and was validated live, reusing an already-shipped,
  already-validated component (`garak-envoy`'s Host-rewrite pattern) rather
  than inventing something new — low incremental risk. It's also now
  chart-driven (values-list of sandboxes, not a hand-written manifest per
  target), so it's genuinely usable for more than one sandbox without
  further engineering — though only the single-target case was exercised
  against a live scrape (see Open items).
- It only becomes useful if there's an actual demo/use case that needs
  Prometheus scraping a sandbox-hosted metric (e.g. a future agent
  observability demo). Nothing in this repo currently consumes it.
- If merged: promote `prometheus-envoy` (or a consolidated
  `host-rewrite-envoy` chart, see above) plus `util/metrics-file-exporter/`,
  and add a real demo section using them (sample metric driven by an
  actual sandbox workload, not a static test file).
- If discarded: the `util/metrics-file-exporter` binary and
  `prometheus-envoy` chart are small and self-contained enough to resurrect
  later from this branch/PR without re-deriving the design.
