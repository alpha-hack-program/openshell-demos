# audit-tempo

A dedicated, demo-owned `TempoMonolithic` instance that stores the traces
[`audit-collector`](../audit-collector/) forwards to it — native OTel
tracing from Claude Code/Codex sandboxes, propagated all the way to the real
MCP servers' Envoy sidecars. See
[`../docs/prometheus-scraping.md`](../docs/prometheus-scraping.md) for the
metrics side of this pipeline (traces are documented alongside it there).

## Prerequisites

The Red Hat Tempo Operator must already be installed on the cluster
(`oc get csv -A | grep tempo-operator`). This chart only creates a
`TempoMonolithic` instance — it doesn't install the operator or its CRDs.

Deliberately separate from this cluster's other `TempoMonolithic`
(`redhat-ods-monitoring/data-science-tempomonolithic`) — that one is
RHOAI-shared infra for a different consumer. Don't point `audit-collector`
at it; each demo owns its own trace store.

## Install

```bash
helm upgrade --install audit-tempo demos/keycloak-oidc/audit-tempo \
  --namespace "$OPENSHELL_NAMESPACE"
```

Install this **before** upgrading `audit-collector`'s traces pipeline to
point at it — `audit-collector`'s `otlphttp/tempo` exporter needs this
instance's Service to already exist and be resolvable.

Confirmed live: this release name (`audit`) produces a Service named
`tempo-audit`, exposing `3200` (Tempo API), `4317` (OTLP grpc), and `4318`
(OTLP http) — that's the endpoint `audit-collector`'s traces exporter should
target. This cluster's only other `TempoMonolithic`
(`data-science-tempomonolithic`) has `multitenancy.enabled: true`, which
produces a different (gateway-only) Service topology — it is not a usable
reference, which is why this was verified directly rather than assumed. If
you rename this release, re-verify with:

```bash
oc get svc -n "$OPENSHELL_NAMESPACE" -l app.kubernetes.io/managed-by=tempo-operator
```

Unlike `audit-collector`, this release's name is **not** load-bearing —
nothing outside this chart hardcodes a hostname for it at compile time.
Renaming is safe as long as you also update `audit-collector`'s traces
exporter endpoint to match.

## Verifying it works

```bash
oc get tempomonolithic -n "$OPENSHELL_NAMESPACE" audit
oc get pods -n "$OPENSHELL_NAMESPACE" -l app.kubernetes.io/managed-by=tempo-operator
```

Once `audit-collector` is wired to push here (see its own README), traces
pushed by a real sandbox turn should be queryable via the Jaeger UI route:

```bash
oc get route -n "$OPENSHELL_NAMESPACE" -l app.kubernetes.io/managed-by=tempo-operator
```

## Values reference

| Value | Default | Description |
|---|---|---|
| `jaegerui.enabled` | `true` | Render the Jaeger UI alongside Tempo — a quick, human-inspectable way to eyeball traces without building anything. Not a substitute for `audit-dashboard`. |
| `jaegerui.route.enabled` | `true` | Expose the Jaeger UI via an OpenShift Route. |

## Known limitations

- **Ephemeral by design**: `storage.traces.backend: memory` (this field's own documented default) means trace history doesn't survive a pod restart. Fine for a demo; not a real retention story.
- **No auth on ingest/query** — confirmed live via the Operator's own install-time warning: "TempoMonolithic instances without multi-tenancy provide no authentication or authorization on the ingest or query paths, and are not supported on OpenShift." Acceptable for a demo whose whole audience is already inside the cluster's own auth boundary; not something to carry into a production pattern without enabling multi-tenancy.
- `spec.jaegerui.enabled` is flagged deprecated by this Tempo Operator version (confirmed live install-time warning) but still functions today — produces a working `tempo-audit-jaegerui` Service + Route. Re-check this field's replacement before the Operator version bumps far enough to actually remove it.
- Single instance, no HA — same tradeoff as `audit-collector`.
- `util/audit-dashboard` does not read from this instance yet — that's separate, out-of-scope follow-up work.
