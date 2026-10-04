#!/usr/bin/env bash
set -euo pipefail
# Publishes a corrected copy of RHOAI's stock MaaS "Usage" Perses
# dashboard, which renders every number as 0 on this cluster.
#
# The stock dashboard queries OpenMetrics-style counters:
#
#   increase(authorized_calls_total{...})   -> Requests
#   increase(authorized_hits_total{...})    -> Tokens
#   increase(limited_calls_total{...})      -> Rate limited
#
# but the Limitador shipped here exposes them WITHOUT the `_total` suffix.
# Confirmed live: `authorized_calls` returns 7 series while
# `authorized_calls_total` returns 0, so every panel evaluates to zero even
# though the data is present and correct. The symptom is badly misleading —
# the table still lists a row per (user, subscription, model), so it reads
# as "these users exist and consumed nothing" rather than "this query
# matched nothing".
#
# This copies the dashboard rather than patching it: the original is owned
# by the maas.opendatahub.io Config CR and reconciled by the
# maas-observability controller, so an in-place edit is reverted. The copy
# drops ownerReferences (otherwise it is garbage-collected with the owner)
# and re-labels managed-by so that controller does not adopt it. Re-running
# re-derives the copy from whatever the stock dashboard currently is, so an
# RHOAI update to the original carries over.
#
# KNOWN LIMIT, not fixed by this script: token panels stay empty for models
# served over the Anthropic Messages route. `authorized_hits` is the
# token-weighted counter, and MaaS only extracts `usage` from
# OpenAI-chat-shaped responses, so e.g. deepseek-flash-anthropic emits
# authorized_calls but never authorized_hits. Per-user *request* accounting
# is therefore complete; per-user *token* accounting only covers the
# OpenAI-format models. The same gap means tokenRateLimits are effectively
# unenforced for Messages-format models.
#
# Usage: ./20-fix-maas-usage-dashboard.sh [--dry-run]

DRY_RUN=false
[[ "${1:-}" == "--dry-run" ]] && DRY_RUN=true

SRC_NS="${MAAS_DASHBOARD_NAMESPACE:-redhat-ods-monitoring}"
SRC_NAME="${MAAS_DASHBOARD_NAME:-dashboard-3-maas-usage-admin}"
DST_NAME="${SRC_NAME}-fixed"

if ! oc get persesdashboard "$SRC_NAME" -n "$SRC_NS" >/dev/null 2>&1; then
  echo "error: stock dashboard $SRC_NS/$SRC_NAME not found." >&2
  echo "       Set MAAS_DASHBOARD_NAMESPACE / MAAS_DASHBOARD_NAME if it moved." >&2
  exit 1
fi

SRC_JSON=$(mktemp --suffix=.json)
DST_JSON=$(mktemp --suffix=.json)
trap 'rm -f "$SRC_JSON" "$DST_JSON"' EXIT
oc get persesdashboard "$SRC_NAME" -n "$SRC_NS" -o json > "$SRC_JSON"

DST_NAME="$DST_NAME" python3 - "$SRC_JSON" "$DST_JSON" <<'PY'
import json, os, sys

src = json.load(open(sys.argv[1]))
dst_name = os.environ["DST_NAME"]

out = {
    "apiVersion": src["apiVersion"],
    "kind": src["kind"],
    "metadata": {
        "name": dst_name,
        "namespace": src["metadata"]["namespace"],
        "labels": {
            "app.kubernetes.io/component": "perses",
            "app.kubernetes.io/part-of": "models-as-a-service",
            "app.opendatahub.io/modelsasservice": "true",
            "app.kubernetes.io/managed-by": "openshell-demos",
        },
    },
    "spec": src["spec"],
}

blob = json.dumps(out)
total = 0
for old, new in (
    ("authorized_calls_total", "authorized_calls"),
    ("authorized_hits_total", "authorized_hits"),
    ("limited_calls_total", "limited_calls"),
    # Second, independent defect: the stock dashboard reads from the Data
    # Science MonitoringStack, whose `namespaceSelector: null` restricts it
    # to its own namespace -- but Limitador's PodMonitor lives in
    # kuadrant-system, so that stack holds none of these counters. Even
    # perfectly named queries return nothing there. The cluster
    # Thanos-querier datasource federates user-workload monitoring, which
    # is where kuadrant-system/kuadrant-limitador-monitor actually lands;
    # verified it returns all 7 authorized_calls series.
    ("data-science-prometheus-datasource", "cluster-prometheus-datasource"),
):
    total += blob.count(old)
    blob = blob.replace(old, new)
out = json.loads(blob)

# Third defect: every panel masks the request counters against
# authorized_hits, because authorized_calls carries no `model` label and
# that mask is how the $model filter gets applied. authorized_hits is the
# TOKEN counter, so any model whose responses MaaS cannot token-count --
# i.e. anything served over the Anthropic Messages route -- is absent from
# it, and its users vanish from every panel even though their requests
# were counted. That is what makes the table look like only one user has a
# key.
#
# Fix: fall back to deriving `model` from limitador_namespace
# ("llm-d-demo/deepseek-flash-anthropic" -> "deepseek-flash-anthropic")
# for any (user, subscription, namespace) with no token hits. `unless`
# keeps the authoritative name where hits do exist, so models like
# alibaba/qwen3-8b are not duplicated under their route name. Verified
# against live data: stock lookup 4 series, this one 7, adding alice, bob
# and charlie on deepseek-flash-anthropic with no qwen duplicates.
def _lookup(by_clause: str) -> str:
    return (
        f'max by ({by_clause}) (max_over_time(authorized_hits{{model=~"$model"}}[$__range]))'
        " or "
        f'max by ({by_clause}) (max_over_time((label_replace('
        'authorized_calls{limitador_namespace=~"[^/]*/($model)"}, '
        '"model", "$1", "limitador_namespace", "[^/]*/(.*)") '
        "unless on(user, subscription, limitador_namespace) authorized_hits)[$__range:]))"
    )

_SUBS = [
    (
        'max by (user, subscription, limitador_namespace, model) '
        '(max_over_time(authorized_hits{model=~"$model"}[$__range]))',
        _lookup("user, subscription, limitador_namespace, model"),
    ),
    (
        'max by (user, subscription, limitador_namespace) '
        '(max_over_time(authorized_hits{model=~"$model"}[$__range]))',
        _lookup("user, subscription, limitador_namespace"),
    ),
]

rewritten = 0
def _walk(node):
    global rewritten
    if isinstance(node, dict):
        for k, v in node.items():
            if k == "query" and isinstance(v, str):
                for old, new in _SUBS:
                    if old in v:
                        rewritten += v.count(old)
                        v = v.replace(old, f"({new})")
                node[k] = v
            else:
                _walk(v)
    elif isinstance(node, list):
        for v in node:
            _walk(v)

_walk(out["spec"]["config"])
print(f"rewrote {rewritten} model-lookup joins")

out["spec"]["config"]["display"] = {
    "name": "Usage (fixed counters)",
    "description": (
        "Copy of the stock MaaS Usage dashboard with counter names corrected "
        "to the unsuffixed series this Limitador exposes. Token panels stay "
        "empty for Anthropic-Messages-format models, which emit "
        "authorized_calls but no authorized_hits."
    ),
}

json.dump(out, open(sys.argv[2], "w"), indent=2)
print(f"rewrote {total} counter references")
if total == 0:
    print("note: no `_total` references found -- upstream may have fixed this "
          "already, in which case this copy is redundant.")
PY

if [ "$DRY_RUN" = true ]; then
  echo "--dry-run: not applying. Rendered manifest:"
  head -20 "$DST_JSON"
  exit 0
fi

oc apply -f "$DST_JSON"
echo
echo "Published $SRC_NS/$DST_NAME alongside the untouched original."
echo "Verify it now returns data (the stock names return nothing):"
echo "  sum by (user, subscription) (increase(authorized_calls{user!=\"\"}[24h]))"
