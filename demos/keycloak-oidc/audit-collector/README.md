# audit-collector

A dedicated `OpenTelemetryCollector` instance that receives OTLP pushes
from [`util/session-auditor`](../../../util/session-auditor/) and
re-exposes them for Prometheus. See
[`../docs/prometheus-scraping.md`](../docs/prometheus-scraping.md) for the
full design.

## Prerequisites

The Red Hat build of OpenTelemetry Operator must already be installed on
the cluster (`oc get csv -A | grep opentelemetry-operator`). This chart
only creates an `OpenTelemetryCollector` instance — it doesn't install the
operator or its CRDs.

## Install

```bash
helm upgrade --install audit demos/keycloak-oidc/audit-collector \
  --namespace "$OPENSHELL_NAMESPACE"
```

**The release name must be `audit`.** The Operator names the generated
Service `<release-name>-collector`, and `audit-collector` is the exact
short, in-namespace DNS name compiled into `session-auditor`'s binary
(`util/session-auditor/otlp-endpoint.txt`). Installing under a different
release name silently breaks the push target — `session-auditor` would
still run, still classify, and still try to push, but the push would fail
to resolve the host.

## Verifying it works

```bash
# Confirm the collector and its Service exist
oc get opentelemetrycollector,svc -n "$OPENSHELL_NAMESPACE" -l app.kubernetes.io/instance="${OPENSHELL_NAMESPACE}.audit"

# Push a test metric directly (bypassing session-auditor) via port-forward
oc port-forward -n "$OPENSHELL_NAMESPACE" svc/audit-collector 4318:4318 &
curl -X POST http://localhost:4318/v1/metrics -H "Content-Type: application/json" -d '{
  "resourceMetrics": [{"scopeMetrics": [{"metrics": [{"name": "test_metric",
    "gauge": {"dataPoints": [{"asDouble": 1, "timeUnixNano": "'"$(date +%s%N)"'"}]}}]}]}]
}'

# Once the ServiceMonitor reconciles (took 30s-3min in live testing — see
# the timing note in prometheus-scraping.md), query it via Prometheus:
oc exec -n openshift-user-workload-monitoring prometheus-user-workload-0 -c prometheus -- \
  wget -qO- 'http://localhost:9090/api/v1/query?query=test_metric'
```

## Values reference

| Value | Default | Description |
|---|---|---|
| `serviceMonitor.enabled` | `true` | Render a `ServiceMonitor` scraping the collector's Prometheus exporter into `openshift-user-workload-monitoring`. |
| `serviceMonitor.interval` | `15s` | Scrape interval. |

## Known limitations

- Single `OpenTelemetryCollector` instance, `mode: deployment` (no HA) — fine for a demo, not sized for production load.
- The ServiceMonitor's selector deliberately excludes the Operator's generated `-headless` and `-monitoring` Services (confirmed live: without this, they produce duplicate scrape targets for the same pod).
