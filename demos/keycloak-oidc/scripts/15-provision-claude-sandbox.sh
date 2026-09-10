#!/usr/bin/env bash
set -euo pipefail
# Provisions one banker's Claude Code sandbox for step 5 of the README
# ("Run the demo") — see "Provision the Claude Code harness" there for the
# narrated walkthrough of what this does and why. Same shape as
# 14-provision-codex-sandbox.sh: a dedicated sandbox named claude-<user-id>
# with both providers (byo-claude for the LLM, user-<id> for MCP calls)
# attached at creation, /sandbox/.claude/mcp-servers.json baked in via
# --upload, and the full claude-code-recipe policy applied at the end.
#
# Usage: ./15-provision-claude-sandbox.sh <user-id> <server-name>[,<server-name>...]
#   e.g. ./15-provision-claude-sandbox.sh bob mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#   e.g. ./15-provision-claude-sandbox.sh alice mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance,mcp-compatibility
#
# Run as admin — provider and policy management stay Platform-Admin
# operations regardless of workspace. Assumes <user-id> was already
# onboarded. Requires ANTHROPIC_API_KEY, ANTHROPIC_BASE_URL, and
# ANTHROPIC_MODEL set in .env.
#
# This script never execs `claude` itself, so it doesn't need the native
# OTel tracing env vars — those are exec-time-only flags added at the
# README's `sandbox exec ... claude` call sites via scripts/lib-otel-env.sh.
# Don't miss that other half when touching this feature.
# Idempotent — provider/profile/sandbox create calls tolerate "already
# exists", and the provider-attach fallback below still attaches
# byo-claude/user-<id> even if the sandbox already existed without them.
#
# SANDBOX_PREFIX (optional, default "") is prepended to the sandbox name
# (<prefix>claude-<user-id>) — set it to provision a second, differently
# named sandbox for the same banker without touching their existing one,
# e.g. for testing this script against a banker who already has a
# claude-<id>. Sandbox names are capped at 19 characters (confirmed live:
# 20 fails with "name exceeds maximum length") — "claude-charlie" alone is
# already 14, so keep any prefix short (5 chars or fewer).

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

SANDBOX_PREFIX="${SANDBOX_PREFIX:-}"
SANDBOX_NAME="${SANDBOX_PREFIX}claude-${USER_ID}"

cd "$DEMO_DIR"

# ---------------------------------------------------------------------------
# Step 1: import the Claude Code provider profile (with <llm-host>
# substituted) and create the provider. Injects ANTHROPIC_API_KEY into the
# sandbox; base URL/model are non-secret config, passed via --env at exec
# time instead (OpenShell only injects credentials, not config values).
# ---------------------------------------------------------------------------
TMPFILE=$(mktemp --suffix=.yaml)
sed "s/<llm-host>/${LLM_HOST}/" providers/byo-claude-profile.yaml > "$TMPFILE"
# `|| true` only tolerates "already exists" on first run — confirmed live
# that `import` against an already-imported profile ID is a hard error, not
# a silent update, so editing this YAML and re-running this script does
# NOT propagate the change to an already-provisioned workspace. To push an
# edit to an existing profile: `provider profile export <id> --workspace
# <ws> -o yaml`, edit, then `provider profile update <id> -f <file>
# --workspace <ws>` (requires the exported resource_version field).
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
  --name "$SANDBOX_NAME"
  --provider byo-claude
  --provider "user-${USER_ID}"
  --upload "${MCP_CONFIG}:/sandbox/.claude/mcp-servers.json"
  --workspace "${USER_ID}"
)
[ -z "$CLAUDE_IMAGE" ] || SANDBOX_CREATE_ARGS+=(--from "$CLAUDE_IMAGE")

# Note: if $SANDBOX_NAME already exists, this is a no-op — the --upload'd
# mcp-servers.json only takes effect at creation time. Re-running with a
# changed server list against an already-provisioned sandbox won't update
# it; delete the sandbox first if you need to change its MCP servers.
#
# The two `provider attach` calls right after are the fallback for that
# same "already existed" case: if $SANDBOX_NAME predates this script (e.g.
# created some other way with only one of the two providers attached),
# `sandbox create ... || true` above silently does nothing — it would
# never actually attach byo-claude/user-<id>, leaving Claude Code with no
# LLM credentials with no error raised. Attaching an already-attached
# provider is a harmless no-op, so these are safe to run unconditionally.
openshell sandbox create "${SANDBOX_CREATE_ARGS[@]}" -- true || true
openshell sandbox provider attach "$SANDBOX_NAME" byo-claude --workspace "${USER_ID}" || true
openshell sandbox provider attach "$SANDBOX_NAME" "user-${USER_ID}" --workspace "${USER_ID}" || true

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
  openshell sandbox exec -n "$SANDBOX_NAME" --workspace "${USER_ID}" -- bash -c \
    'sed -i "s|__USER_ACCESS_TOKEN__|$USER_ACCESS_TOKEN|g" /sandbox/.claude/mcp-servers.json'
  REMAINING=$(openshell sandbox exec -n "$SANDBOX_NAME" --workspace "${USER_ID}" -- \
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
helm template "${SANDBOX_NAME}-policy" policies \
  --set openshellNamespace="${OPENSHELL_NAMESPACE}" \
  --set llmHost="${LLM_HOST}" \
  --set recipe=claude-code \
  --set "mcpServers={${SERVER_NAMES}}" \
  > "${POLICY_TMPFILE}"
openshell policy set "$SANDBOX_NAME" --policy "${POLICY_TMPFILE}" \
  --workspace "${USER_ID}" --wait
rm -f "${POLICY_TMPFILE}"

echo "Claude sandbox $SANDBOX_NAME provisioned in workspace ${USER_ID}, wired to: ${SERVER_NAMES}"
openshell sandbox list --workspace "${USER_ID}"
