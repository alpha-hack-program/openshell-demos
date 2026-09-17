#!/usr/bin/env bash
set -euo pipefail
# Deploys every MCP server listed in mcp-servers/values.yaml's servers:
# array, each expected to validate the caller's Keycloak-issued OAuth
# access token directly. Server names are read back from the rendered
# chart rather than hardcoded, so adding/removing entries in values.yaml
# doesn't require touching this script.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

# Source demo .env for OPENSHELL_NAMESPACE and MCP server image variables
DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"
# Backs mcp-market-news's news_generator initContainer/sidecar (see
# mcp-servers/values.yaml's top-level newsGenerator: block) — passed via
# --set on a top-level values key, never baked into values.yaml or passed
# as servers[N].something, per that file's documented --set list gotcha.
# All three are required with no chart-side default (see values.yaml's
# newsGenerator: comment) so a stale/wrong .env value fails loudly here
# instead of silently falling back to a hardcoded provider.
: "${OPENAI_API_KEY:?set OPENAI_API_KEY in .env (used by mcp-market-news news_generator)}"
: "${OPENAI_BASE_URL:?set OPENAI_BASE_URL in .env (used by mcp-market-news news_generator)}"
: "${OPENAI_MODEL:?set OPENAI_MODEL in .env (used by mcp-market-news news_generator)}"

HELM_SET_ARGS=(
  --set "keycloak.issuer=https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}"
  --set "newsGenerator.openaiApiKey=${OPENAI_API_KEY}"
  --set "newsGenerator.openaiBaseUrl=${OPENAI_BASE_URL}"
  --set "newsGenerator.openaiModel=${OPENAI_MODEL}"
)

# If step 2 created openshell-oidc-ca (self-signed default ingress cert —
# see README's "OIDC issuer TLS trust" section) and OPENAI_BASE_URL points
# at a Route on this same cluster, news_generator needs that same CA to
# trust it. Auto-detected, not required — harmless to skip if
# OPENAI_BASE_URL is a publicly-trusted endpoint instead.
if oc -n "$OPENSHELL_NAMESPACE" get configmap openshell-oidc-ca &>/dev/null; then
  HELM_SET_ARGS+=(--set "newsGenerator.caConfigMapName=openshell-oidc-ca")
fi

helm upgrade --install mcp-servers "$DEMO_DIR/mcp-servers" \
  --namespace "$OPENSHELL_NAMESPACE" \
  "${HELM_SET_ARGS[@]}"

mapfile -t SERVERS < <(
  helm template mcp-servers "$DEMO_DIR/mcp-servers" \
    "${HELM_SET_ARGS[@]}" \
    --show-only templates/serviceaccount.yaml \
    | awk '/^  name: /{print $2}'
)

for s in "${SERVERS[@]}"; do
  oc -n "$OPENSHELL_NAMESPACE" rollout status "deployment/${s}"
done
oc -n "$OPENSHELL_NAMESPACE" rollout status deployment/mcp-postgres 2>/dev/null || true

echo "MCP servers deployed. Reachable in-cluster at:"
for s in "${SERVERS[@]}"; do
  PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$s" -o jsonpath='{.spec.ports[0].port}')
  echo "  ${s}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${PORT}"
done
echo "Next: grant the relevant Keycloak realm role (<server-name>-user) to a"
echo "user, then run"
echo "  ./07-authorize-mcp-user.sh <user-id> <server-name>"
