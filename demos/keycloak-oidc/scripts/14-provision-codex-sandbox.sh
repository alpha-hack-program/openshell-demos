#!/usr/bin/env bash
set -euo pipefail
# Provisions a Codex sandbox for one banker, per Annex A's "Codex + BYO LLM
# + MCP tool" recipe in the README: a workspace-scoped inference.local
# route, a byo-codex provider (network policy limited to
# inference.local:443 + OPENAI_API_KEY injection), the sandbox itself
# (codex-<user-id>, with /sandbox/.codex/config.toml baked in at creation
# time via --upload — no `sandbox exec` needed to configure it), and the
# codex-recipe policy.
#
# Usage: ./14-provision-codex-sandbox.sh [--gw] <user-id> <server-name>[,<server-name>...]
#   e.g. ./14-provision-codex-sandbox.sh bob mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#   e.g. ./14-provision-codex-sandbox.sh alice mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance,mcp-compatibility
#   e.g. ./14-provision-codex-sandbox.sh --gw bob mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#
# --gw (bare flag, default off) routes this sandbox's MCP calls through the
# RHCL MCP Gateway (../mcp-gateway, which must already be deployed with
# auth.enabled) instead of directly to each server's own port-8000 Envoy
# listener — see 15-provision-claude-sandbox.sh's own --gw note for the
# full rationale (config.toml gets one [mcp_servers.gateway] table instead
# of one per server; the policy gets one allow_mcp_gateway group instead
# of one per server).
#
# The README's own example wires up the same server set as the Claude Code
# harness — the two sandboxes are meant to be equivalent, not a narrower
# Codex subset. Each name becomes its own [mcp_servers.<name>] table in
# config.toml and is added to the policy's egress allow-list. Don't
# include mcp-compatibility for anyone but alice — it's gated by the
# compatibility-user realm role.
#
# Run as admin — provider and policy management stay Platform-Admin
# operations regardless of workspace (see "Workspace isolation" in the
# README). Assumes <user-id> was already onboarded via 03-onboard-user.sh
# (their own workspace and a `user-<id>` provider already exist) and that
# Part I steps 1-5 have run. Requires OPENAI_API_KEY, OPENAI_BASE_URL, and
# OPENAI_MODEL set in .env. Idempotent — provider/profile/sandbox create
# calls tolerate "already exists" (matching 03-onboard-user.sh's own
# `|| true` convention) so re-running just re-applies inference set,
# config.toml, and policy against what's already there.
#
# SANDBOX_PREFIX (optional, default "") is prepended to the sandbox name
# (<prefix>codex-<user-id>) — mirrors 15-provision-claude-sandbox.sh's own
# knob, set it to provision a second, differently named sandbox for the
# same banker without touching their existing one (e.g.
# 16-provision-audited-sandbox.sh's aud- prefix). Sandbox names are capped
# at 19 characters (confirmed live in 15-provision-claude-sandbox.sh) —
# "codex-charlie" alone is already 13, so keep any prefix short (6 chars
# or fewer).

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

# Preserved across the `source .env` below — see the same guard in
# 15-provision-claude-sandbox.sh for why: 16-provision-audited-sandbox.sh
# exports CODEX_IMAGE as the *-audit image, and `set -a; source .env`
# would otherwise reassign it back to the plain one, yielding an `aud-`
# sandbox with no session-auditor in it.
_CALLER_CODEX_IMAGE="${CODEX_IMAGE-}"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

[ -n "$_CALLER_CODEX_IMAGE" ] && CODEX_IMAGE="$_CALLER_CODEX_IMAGE"

GW=false
POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --gw) GW=true; shift ;;
    *) POSITIONAL+=("$1"); shift ;;
  esac
done
set -- "${POSITIONAL[@]}"

USER_ID="${1:?usage: $0 [--gw] <user-id> <server-name>[,<server-name>...]}"
SERVER_NAMES="${2:?usage: $0 [--gw] <user-id> <server-name>[,<server-name>...]}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"

if [ "$GW" = true ]; then
  # shellcheck source=lib-mcp-gateway.sh
  source "$SCRIPT_DIR/lib-mcp-gateway.sh"
  MCP_GATEWAY_URL=$(mcp_gateway_url) || exit 1
fi
: "${OPENAI_API_KEY:?set OPENAI_API_KEY in .env}"
: "${OPENAI_BASE_URL:?set OPENAI_BASE_URL in .env}"
: "${OPENAI_MODEL:?set OPENAI_MODEL in .env}"

IFS=',' read -ra SERVERS <<< "$SERVER_NAMES"
[ ${#SERVERS[@]} -gt 0 ] || { echo "no server names given"; exit 1; }

# Codex 0.146.0 — override if the chart's default sandbox image ships an
# older version (see the README's LLM endpoint requirements note: Codex
# 0.146.0+ only supports wire_api = "responses" with namespace tools).
CODEX_IMAGE="${CODEX_IMAGE:-}"

SANDBOX_PREFIX="${SANDBOX_PREFIX:-}"
SANDBOX_NAME="${SANDBOX_PREFIX}codex-${USER_ID}"

cd "$DEMO_DIR"

# ---------------------------------------------------------------------------
# Step 1: inference provider + workspace-scoped inference.local routing.
# Only type `openai` providers can drive inference.local. Runs once per
# banker's workspace — there's no shared/global inference route.
# ---------------------------------------------------------------------------
openshell provider create --name byo-inference --type openai \
  --credential "OPENAI_API_KEY=$OPENAI_API_KEY" \
  --config "OPENAI_BASE_URL=$OPENAI_BASE_URL" \
  --workspace "${USER_ID}" || true

# --no-verify: when the gateway was installed with server.oidc.caConfigMapName
# set (see README's "OIDC issuer TLS trust" section — needed whenever
# Keycloak's Route rides a self-signed default ingress cert), the gateway's
# outbound TLS trust for endpoint verification calls is scoped to that CA
# only, not the public web PKI roots too — verifying a real CA-signed
# endpoint like OPENAI_BASE_URL then fails even though the endpoint itself
# is fine (confirmed live: a raw curl to the same URL succeeds). Endpoint
# reachability is verified by the recipe's own test call later anyway.
openshell inference set \
  --provider byo-inference \
  --model "$OPENAI_MODEL" \
  --timeout 120 \
  --no-verify \
  --workspace "${USER_ID}"

# ---------------------------------------------------------------------------
# Step 2: Codex-specific provider — locks network access to
# inference.local:443 (the privacy router), injects OPENAI_API_KEY, and
# declares the codex binary (see providers/byo-codex-profile.yaml).
# ---------------------------------------------------------------------------
# `|| true` only tolerates "already exists" on first run — confirmed live
# that `import` against an already-imported profile ID is a hard error, not
# a silent update, so editing this YAML and re-running this script does
# NOT propagate the change to an already-provisioned workspace. To push an
# edit to an existing profile: `provider profile export <id> --workspace
# <ws> -o yaml`, edit, then `provider profile update <id> -f <file>
# --workspace <ws>` (requires the exported resource_version field).
openshell provider profile import -f providers/byo-codex-profile.yaml --workspace "${USER_ID}" || true
openshell provider create --name byo-codex --type byo-codex \
  --credential "OPENAI_API_KEY=$OPENAI_API_KEY" \
  --workspace "${USER_ID}" || true

# ---------------------------------------------------------------------------
# Step 3: create the sandbox with /sandbox/.codex/config.toml baked in at
# creation time (model provider + one [mcp_servers.<name>] table per
# server), then apply the codex-recipe policy chart with all of them on
# the egress allow-list.
# ---------------------------------------------------------------------------
CODEX_CONFIG=$(mktemp)
cat > "$CODEX_CONFIG" <<EOF
model_provider = "openshell-byo"
model = "${OPENAI_MODEL}"

[model_providers.openshell-byo]
name = "OpenShell BYO Router"
base_url = "https://inference.local/v1"
env_key = "OPENAI_API_KEY"
wire_api = "responses"

[projects."/sandbox"]
trust_level = "trusted"

# Native OTel tracing (codex-cli 0.146.0+) — config.toml only, no env-var
# equivalent on this version. "protocol" is mandatory: omitting it is a
# hard config-load error ("missing field \`protocol\`"), confirmed live.
# OTEL_RESOURCE_ATTRIBUTES (workspace/sandbox identity) can't go here —
# codex-cli has no TOML field for arbitrary resource attributes yet
# (github.com/openai/codex#30987) — it's passed via --env instead, see
# scripts/lib-otel-env.sh's otel_codex_env_args and README.md's exec blocks.
[otel]

[otel.trace_exporter.otlp-http]
endpoint = "http://audit-collector:4318/v1/traces"
protocol = "binary"
EOF

if [ "$GW" = true ]; then
  cat >> "$CODEX_CONFIG" <<EOF

[mcp_servers.gateway]
url = "${MCP_GATEWAY_URL}/mcp"
bearer_token_env_var = "USER_ACCESS_TOKEN"
EOF
else
  for SERVER in "${SERVERS[@]}"; do
    SERVER_PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$SERVER" -o jsonpath='{.spec.ports[0].port}')
    cat >> "$CODEX_CONFIG" <<EOF

[mcp_servers.${SERVER}]
url = "http://${SERVER}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${SERVER_PORT}/mcp"
bearer_token_env_var = "USER_ACCESS_TOKEN"
EOF
  done
fi

# Render the policy *before* creating the sandbox, so it can be passed to
# `sandbox create --policy` directly, alongside --upload for config.toml,
# in one call. Confirmed live on OpenShell 0.1.2 (see the identical note in
# 15-provision-claude-sandbox.sh): a sandbox created with --provider flags
# but no --policy has no usable policy at all and sits in a
# "ConfigurationInvalid" rejected state until one is set — `sandbox create`
# then blocks client-side waiting for a Ready state that can't arrive
# (~30s to 5 minutes), while the gateway's own independent fail-closed
# safeguard deletes the still-rejected sandbox on its own clock in the
# meantime. A policy-less `sandbox create` reliably loses that race.
# Passing --policy (and --upload) at creation time avoids the race
# entirely — confirmed live (on the Claude Code sandbox; this script's own
# `openshell inference set` call above means it can't be run this far on
# 0.1.2 yet to re-confirm here) that `sandbox create --policy ... --upload
# ...` returns normally, no backgrounding/sleep tricks needed.
POLICY_TMPFILE=$(mktemp --suffix=.yaml)
POLICY_TEMPLATE_ARGS=(
  --set openshellNamespace="${OPENSHELL_NAMESPACE}"
  --set llmHost=inference.local
  --set recipe=codex
)
if [ "$GW" = true ]; then
  GW_HOST_PORT="${MCP_GATEWAY_URL#http://}"
  GW_HOST_PORT="${GW_HOST_PORT#https://}"
  if [[ "$GW_HOST_PORT" == *:* ]]; then
    GW_PORT="${GW_HOST_PORT##*:}"
  else
    GW_PORT=443
  fi
  POLICY_TEMPLATE_ARGS+=(
    --set "mcpGatewayHost=${GW_HOST_PORT%%:*}"
    --set "mcpGatewayPort=${GW_PORT}"
  )
else
  POLICY_TEMPLATE_ARGS+=(--set "mcpServers={${SERVER_NAMES}}")
fi
helm template "${SANDBOX_NAME}-policy" policies "${POLICY_TEMPLATE_ARGS[@]}" \
  > "${POLICY_TMPFILE}"

# Note: if $SANDBOX_NAME already exists, this is a no-op — the --policy and
# --upload'd config.toml only take effect at creation time. Re-running with
# a changed server list against an already-provisioned sandbox won't
# update it; delete the sandbox first if you need to change its MCP
# servers or policy.
openshell sandbox create --name "$SANDBOX_NAME" \
  --provider byo-codex \
  --provider "user-${USER_ID}" \
  --from "${CODEX_IMAGE}" \
  --policy "$POLICY_TMPFILE" \
  --upload "${CODEX_CONFIG}:/sandbox/.codex/config.toml" \
  --workspace "${USER_ID}" \
  --detach || true
rm -f "${POLICY_TMPFILE}" "$CODEX_CONFIG"

if [ "$GW" = true ]; then
  echo "Codex sandbox $SANDBOX_NAME provisioned in workspace ${USER_ID}, routed through the MCP Gateway (${MCP_GATEWAY_URL}) to: ${SERVER_NAMES}"
else
  echo "Codex sandbox $SANDBOX_NAME provisioned in workspace ${USER_ID}, wired to: ${SERVER_NAMES}"
fi
openshell sandbox list --workspace "${USER_ID}"
