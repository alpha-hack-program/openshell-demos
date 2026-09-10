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

[`../audit-tempo`](../audit-tempo/) must already be installed — this
collector's `traces` pipeline forwards to its `tempo-audit` Service.

## Install

```bash
helm upgrade --install audit-tempo demos/keycloak-oidc/audit-tempo \
  --namespace "$OPENSHELL_NAMESPACE"
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

Confirm the compliance-risk alert fires (Thanos Ruler, not Prometheus, evaluates
`PrometheusRule` alerts for user-workload namespaces — confirmed live via its
own API, port 9091 behind the standard cluster-monitoring auth):

```bash
oc port-forward -n openshift-user-workload-monitoring svc/thanos-ruler 9091:9091 &
curl -sk -H "Authorization: Bearer $(oc whoami --show-token)" \
  'https://localhost:9091/api/v1/alerts' | jq '.data.Alerts[] | select(.labels.alertname=="SessionComplianceRiskDetected")'
```
Confirmed live: a real forced-overreach turn (`session_compliance_risk_score`
reaching `2`) produced a `state: firing` entry here within ~15s, which then
reached the platform `alertmanager-main` (same auth pattern, port 9094) with
`status.state: active` — visible in the OpenShift console's own
Observe → Alerting page with no further wiring needed.

## Values reference

| Value | Default | Description |
|---|---|---|
| `serviceMonitor.enabled` | `true` | Render a `ServiceMonitor` scraping the collector's Prometheus exporter into `openshift-user-workload-monitoring`. |
| `serviceMonitor.interval` | `15s` | Scrape interval. |
| `alerting.enabled` | `true` | Render a `PrometheusRule` alerting on `session_compliance_risk_score`. |
| `alerting.riskThreshold` | `2` | Score threshold the alert fires at — `2` matches `blocked_attempt` or worse (see `util/session-auditor/prompt.txt`'s score scheme). |
| `alerting.severity` | `warning` | The alert's `severity` label. |
| `alerting.forDuration` | `0m` | How long the condition must hold before firing — `0m` fires immediately, matching this metric's own "current state, not a blip" design (see `util/session-auditor/README.md`). |

## Known limitations

- Single `OpenTelemetryCollector` instance, `mode: deployment` (no HA) — fine for a demo, not sized for production load.
- The ServiceMonitor's selector deliberately excludes the Operator's generated `-headless` and `-monitoring` Services (confirmed live: without this, they produce duplicate scrape targets for the same pod).
- **The alert reaches Alertmanager but only the default catch-all
  receiver.** Confirmed live: `enableUserWorkload: true` (already set on
  this cluster) is enough for the `PrometheusRule` to be evaluated and for
  the resulting alert to show up in the OpenShift console's own
  Observe → Alerting page — no extra config needed for that. Routing it
  somewhere specific (Slack, email, a webhook) needs a namespace-scoped
  `AlertmanagerConfig`, which additionally requires
  `enableUserAlertmanagerConfig: true` in the *cluster-wide*
  `cluster-monitoring-config` ConfigMap in `openshift-monitoring` —
  confirmed live this is **not** set on this cluster. Enabling it is a
  platform-admin decision affecting every namespace's alerting posture,
  not something this chart can or should turn on by itself.
