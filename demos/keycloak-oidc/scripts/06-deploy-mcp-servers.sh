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
#
# PLATFORM_OPENAI_* takes precedence over OPENAI_*, because this generator
# is a *service*, not a person: it regenerates a batch every few minutes
# for as long as the demo is up, and on a MaaS cluster every one of those
# calls is billed to whoever owns the key. Pointed at a banker's key it
# accrues orders of magnitude more spend than any human in the demo and
# swamps the Usage dashboard's per-user panels (see dashboards/README.md).
# Each falls back to its OPENAI_* equivalent so an existing .env keeps
# working -- the host is usually the same MaaS gateway, but a separate
# platform subscription may publish a different model, so all three are
# overridable independently rather than just the key.
PLATFORM_OPENAI_API_KEY="${PLATFORM_OPENAI_API_KEY:-${OPENAI_API_KEY:-}}"
PLATFORM_OPENAI_BASE_URL="${PLATFORM_OPENAI_BASE_URL:-${OPENAI_BASE_URL:-}}"
PLATFORM_OPENAI_MODEL="${PLATFORM_OPENAI_MODEL:-${OPENAI_MODEL:-}}"
: "${PLATFORM_OPENAI_API_KEY:?set PLATFORM_OPENAI_API_KEY (preferred) or OPENAI_API_KEY in .env (used by mcp-market-news news_generator)}"
: "${PLATFORM_OPENAI_BASE_URL:?set PLATFORM_OPENAI_BASE_URL (preferred) or OPENAI_BASE_URL in .env (used by mcp-market-news news_generator)}"
: "${PLATFORM_OPENAI_MODEL:?set PLATFORM_OPENAI_MODEL (preferred) or OPENAI_MODEL in .env (used by mcp-market-news news_generator)}"

if [ "${PLATFORM_OPENAI_API_KEY}" = "${OPENAI_API_KEY:-}" ]; then
  echo "news-generator: using OPENAI_API_KEY (no PLATFORM_OPENAI_API_KEY set) --" \
       "its MaaS spend will be billed to that key's owner."
else
  echo "news-generator: using PLATFORM_OPENAI_API_KEY."
fi

HELM_SET_ARGS=(
  --set "keycloak.issuer=https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}"
  --set "newsGenerator.openaiApiKey=${PLATFORM_OPENAI_API_KEY}"
  --set "newsGenerator.openaiBaseUrl=${PLATFORM_OPENAI_BASE_URL}"
  --set "newsGenerator.openaiModel=${PLATFORM_OPENAI_MODEL}"
  # Set this when the generator's model is a reasoning model — see
  # mcp-servers/values.yaml's newsGenerator.disableThinking comment for
  # what goes wrong without it (intermittent init CrashLoop, "could not
  # find a JSON array or object in LLM response: <think>").
  --set "newsGenerator.disableThinking=${NEWS_GENERATION_DISABLE_THINKING:-false}"
)

# If step 2 created openshell-oidc-ca (self-signed default ingress cert —
# see README's "OIDC issuer TLS trust" section) and the generator's base
# URL points at a Route on this same cluster, news_generator needs that
# same CA to trust it. Auto-detected, not required — harmless to skip if
# that URL is a publicly-trusted endpoint instead.
if oc -n "$OPENSHELL_NAMESPACE" get configmap openshell-oidc-ca &>/dev/null; then
  HELM_SET_ARGS+=(--set "newsGenerator.caConfigMapName=openshell-oidc-ca")
fi

# RHCL MCP Gateway integration (optional). When MCP_GATEWAY_ENABLED=true,
# the chart creates per-server HTTPRoute + MCPServerRegistration + a
# ReferenceGrant in this namespace. The Gateway/MCPGatewayExtension
# themselves come from ../mcp-gateway (a separate, cluster-scoped chart —
# see its README) — install that once per cluster, before or after this
# script, rather than patching anything by hand.
if [[ "${MCP_GATEWAY_ENABLED:-false}" == "true" ]]; then
  : "${MCP_GATEWAY_NAME:=mcp-gateway}"
  : "${MCP_GATEWAY_NAMESPACE:=openshift-ingress}"
  : "${MCP_GATEWAY_SECTION:=mcp}"
  : "${MCP_GATEWAY_REG_NAMESPACE:=mcp-gateway-system}"
  : "${MCP_GATEWAY_INTERNAL_DOMAIN:=mcp.local}"
  : "${MCP_GATEWAY_PORT:=8010}"
  HELM_SET_ARGS+=(
    --set "mcpGateway.enabled=true"
    --set "mcpGateway.gatewayName=${MCP_GATEWAY_NAME}"
    --set "mcpGateway.gatewayNamespace=${MCP_GATEWAY_NAMESPACE}"
    --set "mcpGateway.sectionName=${MCP_GATEWAY_SECTION}"
    --set "mcpGateway.registrationNamespace=${MCP_GATEWAY_REG_NAMESPACE}"
    --set "mcpGateway.internalDomain=${MCP_GATEWAY_INTERNAL_DOMAIN}"
    --set "mcpGateway.gatewayPort=${MCP_GATEWAY_PORT}"
  )
  echo "MCP Gateway integration enabled — HTTPRoute + MCPServerRegistration will be created."

  # MCP Gateway JWT auth (optional, requires Kuadrant + Authorino).
  # When MCP_GATEWAY_AUTH_ENABLED=true, the chart creates the authn+authz
  # AuthPolicy pair described in values.yaml's mcpGateway.auth comment.
  # Install ../mcp-gateway with auth.enabled=true (its default) BEFORE
  # enabling this — it creates the required mcp-gateway-authn-ssl
  # EnvoyFilter and the two-listener Gateway these AuthPolicies target.
  if [[ "${MCP_GATEWAY_AUTH_ENABLED:-false}" == "true" ]]; then
    HELM_SET_ARGS+=(
      --set "mcpGateway.auth.enabled=true"
    )
    echo "MCP Gateway auth enabled — AuthPolicy on mcp-gateway-route will be created."
  fi
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
