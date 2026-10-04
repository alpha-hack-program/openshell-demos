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
):
    total += blob.count(old)
    blob = blob.replace(old, new)
out = json.loads(blob)

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
