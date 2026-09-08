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
# Usage: ./14-provision-codex-sandbox.sh <user-id> <server-name>[,<server-name>...]
#   e.g. ./14-provision-codex-sandbox.sh bob mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#   e.g. ./14-provision-codex-sandbox.sh alice mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance,mcp-compatibility
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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

USER_ID="${1:?usage: $0 <user-id> <server-name>[,<server-name>...]}"
SERVER_NAMES="${2:?usage: $0 <user-id> <server-name>[,<server-name>...]}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${OPENAI_API_KEY:?set OPENAI_API_KEY in .env}"
: "${OPENAI_BASE_URL:?set OPENAI_BASE_URL in .env}"
: "${OPENAI_MODEL:?set OPENAI_MODEL in .env}"

IFS=',' read -ra SERVERS <<< "$SERVER_NAMES"
[ ${#SERVERS[@]} -gt 0 ] || { echo "no server names given"; exit 1; }

# Codex 0.146.0 — override if the chart's default sandbox image ships an
# older version (see the README's LLM endpoint requirements note: Codex
# 0.146.0+ only supports wire_api = "responses" with namespace tools).
CODEX_IMAGE="${CODEX_IMAGE:-}"

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

openshell inference set \
  --provider byo-inference \
  --model "$OPENAI_MODEL" \
  --timeout 120 \
  --workspace "${USER_ID}"

# ---------------------------------------------------------------------------
# Step 2: Codex-specific provider — locks network access to
# inference.local:443 (the privacy router), injects OPENAI_API_KEY, and
# declares the codex binary (see providers/byo-codex-profile.yaml).
# ---------------------------------------------------------------------------
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
EOF

for SERVER in "${SERVERS[@]}"; do
  SERVER_PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$SERVER" -o jsonpath='{.spec.ports[0].port}')
  cat >> "$CODEX_CONFIG" <<EOF

[mcp_servers.${SERVER}]
url = "http://${SERVER}.${OPENSHELL_NAMESPACE}.svc.cluster.local:${SERVER_PORT}/mcp"
bearer_token_env_var = "USER_ACCESS_TOKEN"
EOF
done

# Note: if codex-${USER_ID} already exists, this is a no-op — the
# --upload'd config.toml only takes effect at creation time. Re-running
# with a changed server list against an already-provisioned sandbox won't
# update it; delete the sandbox first if you need to change its MCP
# servers.
openshell sandbox create --name "codex-${USER_ID}" \
  --provider byo-codex \
  --provider "user-${USER_ID}" \
  --from "${CODEX_IMAGE}" \
  --upload "${CODEX_CONFIG}:/sandbox/.codex/config.toml" \
  --workspace "${USER_ID}" \
  -- true || true

rm -f "$CODEX_CONFIG"

POLICY_TMPFILE=$(mktemp --suffix=.yaml)
helm template "codex-${USER_ID}-policy" policies \
  --set openshellNamespace="${OPENSHELL_NAMESPACE}" \
  --set llmHost=inference.local \
  --set recipe=codex \
  --set "mcpServers={${SERVER_NAMES}}" \
  > "${POLICY_TMPFILE}"
openshell policy set "codex-${USER_ID}" --policy "${POLICY_TMPFILE}" \
  --workspace "${USER_ID}" --wait
rm -f "${POLICY_TMPFILE}"

echo "Codex sandbox codex-${USER_ID} provisioned in workspace ${USER_ID}, wired to: ${SERVER_NAMES}"
openshell sandbox list --workspace "${USER_ID}"
