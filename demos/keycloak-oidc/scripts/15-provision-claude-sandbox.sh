#!/usr/bin/env bash
set -euo pipefail
# Provisions a dedicated Claude Code sandbox for one banker, mirroring
# 14-provision-codex-sandbox.sh's shape: its own sandbox named
# claude-<user-id> (rather than attaching to the existing demo-<user-id>
# the way the README's own recipes do — Annex A's "Claude Code + BYO LLM
# + MCP tool" and "Provision the Claude Code harness" both reuse
# demo-<id>, since Claude Code is pre-installed in the base sandbox image
# and demo-<id> already carries the real user-<id> credential). This
# script deliberately provisions a separate, disposable claude-<id>
# sandbox instead, for parity with how Codex gets its own.
#
# Usage: ./15-provision-claude-sandbox.sh <user-id> <server-name>[,<server-name>...]
#   e.g. ./15-provision-claude-sandbox.sh bob mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#   e.g. ./15-provision-claude-sandbox.sh alice mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance,mcp-compatibility
#
# Run as admin — provider and policy management stay Platform-Admin
# operations regardless of workspace. Assumes <user-id> was already
# onboarded via 03-onboard-user.sh (their own workspace and a `user-<id>`
# provider already exist — the latter is attached at sandbox-create time
# below, exactly like 14 attaches `user-<id>` for Codex). Requires
# ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL, and ANTHROPIC_MODEL set in .env.
# Idempotent — provider/profile/sandbox create calls tolerate "already
# exists" (matching 03-onboard-user.sh's own `|| true` convention) so
# re-running just re-applies the token substitution and policy against
# what's already there.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

USER_ID="${1:?usage: $0 <user-id> <server-name>[,<server-name>...]}"
SERVER_NAMES="${2:?usage: $0 <user-id> <server-name>[,<server-name>...]}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY in .env}"
: "${ANTHROPIC_BASE_URL:?set ANTHROPIC_BASE_URL in .env}"
: "${ANTHROPIC_MODEL:?set ANTHROPIC_MODEL in .env}"

IFS=',' read -ra SERVERS <<< "$SERVER_NAMES"
[ ${#SERVERS[@]} -gt 0 ] || { echo "no server names given"; exit 1; }

# In 0.0.106 the proxy only injects credentials for matching endpoints, so
# the provider profile's <llm-host> placeholder must match the real host.
LLM_HOST=$(echo "$ANTHROPIC_BASE_URL" | sed 's|https\?://||;s|/.*||')

# Claude Code is pre-installed in the chart's default sandbox image
# (unlike Codex, which needs a newer custom image) — leave unset unless
# you specifically need a different one.
CLAUDE_IMAGE="${CLAUDE_IMAGE:-}"

cd "$DEMO_DIR"

# ---------------------------------------------------------------------------
# Step 1: import the Claude Code provider profile (with <llm-host>
# substituted) and create the provider. Injects ANTHROPIC_API_KEY into the
# sandbox; base URL/model are non-secret config, passed via --env at exec
# time instead (OpenShell only injects credentials, not config values).
# ---------------------------------------------------------------------------
TMPFILE=$(mktemp --suffix=.yaml)
sed "s/<llm-host>/${LLM_HOST}/" providers/byo-claude-profile.yaml > "$TMPFILE"
openshell provider profile import -f "$TMPFILE" --workspace "${USER_ID}" || true
rm -f "$TMPFILE"

openshell provider create --name byo-claude --type byo-claude \
  --credential "ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY" \
  --workspace "${USER_ID}" || true

# ---------------------------------------------------------------------------
# Step 2: build /sandbox/.claude/mcp-servers.json locally (one entry per
# server, keyed by its short name with the "mcp-" prefix stripped — Claude's
# tool names come out as mcp__<key>__<tool>, e.g.
# mcp__portfolio__list_my_clients — the URL still uses the real service DNS
# name) with a __USER_ACCESS_TOKEN__ placeholder, then create the sandbox
# with it baked in via --upload, same as 14 bakes in Codex's config.toml.
# The real token isn't known yet — user-<id> (which injects it as
# $USER_ACCESS_TOKEN) is only attached once the sandbox exists — so this
# is still a placeholder at creation time; step 3 substitutes it after.
# ---------------------------------------------------------------------------
MCP_CONFIG=$(mktemp --suffix=.json)
{
  printf '{"mcpServers":{'
  FIRST=1
  for SERVER in "${SERVERS[@]}"; do
    SERVER_PORT=$(oc -n "$OPENSHELL_NAMESPACE" get svc "$SERVER" -o jsonpath='{.spec.ports[0].port}')
    KEY="${SERVER#mcp-}"
    [ "$FIRST" -eq 1 ] || printf ','
    FIRST=0
    printf '"%s":{"type":"http","url":"http://%s.%s.svc.cluster.local:%s/mcp","headers":{"Authorization":"Bearer __USER_ACCESS_TOKEN__"}}' \
      "$KEY" "$SERVER" "$OPENSHELL_NAMESPACE" "$SERVER_PORT"
  done
  printf '}}'
} > "$MCP_CONFIG"

SANDBOX_CREATE_ARGS=(
  --name "claude-${USER_ID}"
  --provider byo-claude
  --provider "user-${USER_ID}"
  --upload "${MCP_CONFIG}:/sandbox/.claude/mcp-servers.json"
  --workspace "${USER_ID}"
)
[ -z "$CLAUDE_IMAGE" ] || SANDBOX_CREATE_ARGS+=(--from "$CLAUDE_IMAGE")

# Note: if claude-${USER_ID} already exists, this is a no-op — the
# --upload'd mcp-servers.json only takes effect at creation time.
# Re-running with a changed server list against an already-provisioned
# sandbox won't update it; delete the sandbox first if you need to change
# its MCP servers.
openshell sandbox create "${SANDBOX_CREATE_ARGS[@]}" -- true || true

rm -f "$MCP_CONFIG"

# ---------------------------------------------------------------------------
# Step 3: substitute the real $USER_ACCESS_TOKEN into the uploaded file.
# Don't construct the JSON inside `sandbox exec` in the first place (a
# printf/heredoc nested in `bash -c '...'` is fragile — one missed
# %s/argument pair silently breaks the JSON, and heredocs reliably hang
# there), and don't relay the token out of the sandbox and back in
# (observed live to silently return empty on a cold connection, producing
# a blank Bearer header with no error) — see "Write each banker's MCP
# server config once" in the README.
#
# A `sandbox exec` run immediately after `sandbox upload`/`sandbox create`
# has also been observed to silently no-op on a cold connection (neither
# has a --wait flag). The README works around this by looping over
# multiple bankers between the upload and substitution steps, letting the
# connection settle; since this script only handles one banker, it
# verifies the substitution took (grep for the placeholder) and retries
# instead.
# ---------------------------------------------------------------------------
REMAINING=""
for attempt in 1 2 3; do
  openshell sandbox exec -n "claude-${USER_ID}" --workspace "${USER_ID}" -- bash -c \
    'sed -i "s|__USER_ACCESS_TOKEN__|$USER_ACCESS_TOKEN|g" /sandbox/.claude/mcp-servers.json'
  REMAINING=$(openshell sandbox exec -n "claude-${USER_ID}" --workspace "${USER_ID}" -- \
    grep -o __USER_ACCESS_TOKEN__ /sandbox/.claude/mcp-servers.json || true)
  [ -z "$REMAINING" ] && break
  echo "token substitution didn't take (attempt ${attempt}/3, likely the post-create cold-connection race) — retrying..."
  sleep 2
done
[ -z "$REMAINING" ] || { echo "failed to substitute USER_ACCESS_TOKEN into mcp-servers.json after 3 attempts"; exit 1; }

# ---------------------------------------------------------------------------
# Step 4: apply the full policy via the same Helm chart 14 uses for Codex
# (recipe=claude-code instead of recipe=codex).
# ---------------------------------------------------------------------------
POLICY_TMPFILE=$(mktemp --suffix=.yaml)
helm template "claude-${USER_ID}-policy" policies \
  --set openshellNamespace="${OPENSHELL_NAMESPACE}" \
  --set llmHost="${LLM_HOST}" \
  --set recipe=claude-code \
  --set "mcpServers={${SERVER_NAMES}}" \
  > "${POLICY_TMPFILE}"
openshell policy set "claude-${USER_ID}" --policy "${POLICY_TMPFILE}" \
  --workspace "${USER_ID}" --wait
rm -f "${POLICY_TMPFILE}"

echo "Claude sandbox claude-${USER_ID} provisioned in workspace ${USER_ID}, wired to: ${SERVER_NAMES}"
openshell sandbox list --workspace "${USER_ID}"
