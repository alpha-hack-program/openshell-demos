#!/usr/bin/env bash
set -euo pipefail
# Deploys the openclaw chart. Prerequisites, none scripted here:
#   1. The `openshell-openclaw-proxy` Keycloak client must already exist
#      (realm-export.rendered.json, imported in step 1c — see
#      scripts/01-deploy-keycloak.sh, which renders and generates its
#      client secret).
#   2. scripts/12-bootstrap-openclaw-alice-session.sh must have already run
#      and its output Secret (openclaw-alice-session) must exist in
#      $OPENSHELL_NAMESPACE.
#   3. The openclaw-oauth2-proxy-secrets and openclaw-secrets Secrets must
#      already exist — see ../openclaw/README.md and the instructions
#      printed by scripts/01-deploy-keycloak.sh.
#   4. The custom image (../images/openclaw-openshell/) must be built and
#      pushed, with values.yaml's image.repository/tag updated to match.
#
# OPENCLAW_ROUTE_HOST here is openclaw's OWN route (not the gateway's). By
# convention it's openclaw-<namespace>.<apps-domain>, derived below the
# same way ROUTE_HOST is in step 2a — this MUST match the redirectUri host
# baked into the openshell-openclaw-proxy Keycloak client, which
# scripts/01-deploy-keycloak.sh renders into keycloak/realm-export.rendered.json
# using the exact same formula before the realm is imported in step 1c.
# Override by exporting OPENCLAW_ROUTE_HOST yourself if you rendered the
# realm with a different value.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
OPENCLAW_ROUTE_HOST="${OPENCLAW_ROUTE_HOST:-openclaw-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}}"
ROUTE_HOST="${ROUTE_HOST:-openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}}"

for secret in openclaw-alice-session openclaw-oauth2-proxy-secrets openclaw-secrets; do
  if ! oc -n "$OPENSHELL_NAMESPACE" get secret "$secret" >/dev/null 2>&1; then
    echo "Missing Secret '$secret' in namespace $OPENSHELL_NAMESPACE." >&2
    echo "See ../openclaw/README.md for how to create it." >&2
    exit 1
  fi
done

# ghcr.io/openclaw/openclaw hardcodes /home/node/.openclaw to 0700 owned by
# uid=1000/gid=1000 (confirmed by inspecting the image) — it doesn't follow
# OpenShift's arbitrary-UID convention the way OpenShell's own images do.
# The chart's Deployment pins securityContext.runAsUser: 1000; this SCC
# grant is what actually lets OpenShift accept that fixed UID. `anyuid`
# (not `privileged` — narrower, same shape as the openshell-sandbox grant
# in demos/base/README.md) must exist on the ServiceAccount before the pod
# is admitted. `oc adm policy add-scc-to-user -z <name>` grants by SA name
# (a RoleBinding subject string) — it does NOT require the SA object to
# already exist, so don't pre-create it here: the chart's own
# templates/serviceaccount.yaml creates it, and Helm needs to be the one
# to create that object or it refuses to adopt it ("exists and cannot be
# imported into the current release" — confirmed live, 2026-09-07).
oc adm policy add-scc-to-user anyuid -z openclaw -n "$OPENSHELL_NAMESPACE"

# Read the LLM API key straight out of the already-created openclaw-secrets
# Secret (never from a committed file) and pass it through to the chart's
# openclaw-config Secret via --set-string. See ../openclaw/values.yaml's
# llm.apiKey/llm.baseUrl comment for why this is the reliable path instead
# of an env var.
LLM_API_KEY=$(oc -n "$OPENSHELL_NAMESPACE" get secret openclaw-secrets -o jsonpath='{.data.ANTHROPIC_API_KEY}' | base64 -d)
LLM_BASE_URL="${ANTHROPIC_BASE_URL:-}"

helm upgrade --install openclaw "$DEMO_DIR/openclaw" \
  --namespace "$OPENSHELL_NAMESPACE" \
  --set "route.host=${OPENCLAW_ROUTE_HOST}" \
  --set "gatewayRouteHost=${ROUTE_HOST}" \
  --set "keycloak.host=${KEYCLOAK_HOST}" \
  --set "keycloak.realm=${KEYCLOAK_REALM}" \
  --set-string "llm.apiKey=${LLM_API_KEY}" \
  --set-string "llm.baseUrl=${LLM_BASE_URL}"

oc -n "$OPENSHELL_NAMESPACE" rollout status deployment/openclaw

echo "openclaw deployed: https://${OPENCLAW_ROUTE_HOST}/"
echo "Log in as alice — she must already be the identity packaged into the"
echo "openclaw-alice-session Secret."
