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

**User-workload-monitoring must be enabled**, or the `ServiceMonitor` this
chart creates has no Prometheus instance to scrape it at all — the metrics
sit in the collector's own Prometheus exporter forever, never reaching
Thanos-querier, and `audit-dashboard`'s graph stays permanently empty with
no error anywhere to point at the cause. Not every cluster has this on by
default — check first:

```bash
oc get pods -n openshift-user-workload-monitoring
```

If that namespace doesn't exist or has no pods, enable it (safe to apply
even if `cluster-monitoring-config` doesn't exist yet — this creates it
fresh rather than overwriting existing cluster monitoring settings; if it
already exists, merge `enableUserWorkload: true` into its `data.config.yaml`
instead of replacing the whole ConfigMap):

```bash
oc apply -f - <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: cluster-monitoring-config
  namespace: openshift-monitoring
data:
  config.yaml: |
    enableUserWorkload: true
EOF
```

Wait for `prometheus-user-workload-0` and `thanos-ruler-user-workload-0`
to reach `Running` (a minute or two) before deploying this chart.

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

Confirm the compliance-risk alert fires (Thanos Ruler, not Prometheus,
evaluates `PrometheusRule` alerts for user-workload namespaces — query its
own API, port 9091, behind the standard cluster-monitoring auth):

```bash
oc port-forward -n openshift-user-workload-monitoring svc/thanos-ruler 9091:9091 &
curl -sk -H "Authorization: Bearer $(oc whoami --show-token)" \
  'https://localhost:9091/api/v1/alerts' | jq '.data.Alerts[] | select(.labels.alertname=="SessionComplianceRiskDetected")'
```
A forced-overreach turn (`session_compliance_risk_score` reaching `2`)
produces a `state: firing` entry here within about 15s, which then reaches
the platform `alertmanager-main` (same auth pattern, port 9094) with
`status.state: active` — visible in the OpenShift console's own
Observe → Alerting page with no further wiring needed.

## Managing this alert (SRE lifecycle)

**It doesn't just silently resolve itself before anyone looks.** By
default, this kind of alert would clear the instant the underlying metric
goes stale (sandbox idle, ~5m) or a later turn scores below threshold —
for a compliance signal, that's a real risk: a flagged incident could
vanish from the active-alerts list with nobody having actually seen it.
`alerting.keepFiringFor` (default `15m`) holds the alert visibly firing
for at least that long regardless, so an SRE has a real window to notice
and act on it rather than it resolving by neglect. Setting
`keep_firing_for` on the `PrometheusRule` object takes a normal
reconciliation cycle (tens of seconds, not instant) before the actual
rule file Thanos Ruler evaluates picks it up — check with the "Confirming
it works" commands above if it doesn't seem to be applying yet.

**Explicit silencing is the right tool for "I've seen this, stop paging
me" — not automation.** Don't try to make the alert auto-suppress itself;
that defeats the point of having a compliance signal at all. The normal
SRE workflow is a time-bound Alertmanager silence, created only after a
human has actually looked at the incident:

```bash
# Via the console: open the alert in Observe -> Alerting, click "Silence
# alert", set a duration and an optional comment.

# Via the API (same auth pattern as above, port 9094):
oc port-forward -n openshift-monitoring svc/alertmanager-main 9094:9094 &
curl -sk -H "Authorization: Bearer $(oc whoami --show-token)" \
  -H "Content-Type: application/json" \
  -X POST 'https://localhost:9094/api/v2/silences' \
  -d '{
    "matchers": [
      {"name": "alertname", "value": "SessionComplianceRiskDetected", "isRegex": false},
      {"name": "sandbox", "value": "aud-claude-bob", "isRegex": false}
    ],
    "startsAt": "'"$(date -u +%Y-%m-%dT%H:%M:%S.000Z)"'",
    "endsAt": "'"$(date -u -d '+4 hours' +%Y-%m-%dT%H:%M:%S.000Z)"'",
    "createdBy": "your-name",
    "comment": "Investigating — see ticket XYZ-123"
  }'
```

Scope the `matchers` to the specific `sandbox` (or `workspace`) under
investigation, not the bare `alertname` — a blanket silence on the
alertname would suppress the signal for every sandbox in the namespace,
not just the one being looked at.

## Values reference

| Value | Default | Description |
|---|---|---|
| `serviceMonitor.enabled` | `true` | Render a `ServiceMonitor` scraping the collector's Prometheus exporter into `openshift-user-workload-monitoring`. |
| `serviceMonitor.interval` | `15s` | Scrape interval. |
| `alerting.enabled` | `true` | Render a `PrometheusRule` alerting on `session_compliance_risk_score`. |
| `alerting.riskThreshold` | `2` | Score threshold the alert fires at — `2` matches `blocked_attempt` or worse (see `util/session-auditor/prompt.txt`'s score scheme). |
| `alerting.severity` | `warning` | The alert's `severity` label. |
| `alerting.forDuration` | `0m` | How long the condition must hold before firing — `0m` fires immediately, matching this metric's own "current state, not a blip" design (see `util/session-auditor/README.md`). |
| `alerting.keepFiringFor` | `15m` | Minimum time the alert stays visibly firing even after the score drops or the metric goes stale — see "Managing this alert" below for why. |

## Known limitations

- Single `OpenTelemetryCollector` instance, `mode: deployment` (no HA) — fine for a demo, not sized for production load.
- The ServiceMonitor's selector deliberately excludes the Operator's generated `-headless` and `-monitoring` Services — without this, they produce duplicate scrape targets for the same pod.
- **The alert reaches Alertmanager but only the default catch-all
  receiver.** With user-workload-monitoring enabled (see Prerequisites
  above), the `PrometheusRule` gets evaluated and the resulting alert
  shows up in the OpenShift console's own Observe → Alerting page — no
  extra config needed for that. Routing it somewhere specific (Slack,
  email, a webhook) needs a namespace-scoped `AlertmanagerConfig`, which
  additionally requires `enableUserAlertmanagerConfig: true` in the
  *cluster-wide* `cluster-monitoring-config` ConfigMap in
  `openshift-monitoring` — off by default. Enabling it is a
  platform-admin decision affecting every namespace's alerting posture,
  not something this chart can or should turn on by itself.
