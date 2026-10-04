#!/usr/bin/env bash
set -euo pipefail
# Provisions one banker's Claude Code sandbox for step 5 of the README
# ("Run the demo") — see "Provision the Claude Code harness" there for the
# narrated walkthrough of what this does and why. Same shape as
# 14-provision-codex-sandbox.sh: a dedicated sandbox named claude-<user-id>
# with both providers (byo-claude for the LLM, user-<id> for MCP calls),
# the full claude-code-recipe policy, and /sandbox/.claude/mcp-servers.json
# all passed to a single `sandbox create --provider --provider --policy
# --upload` call (required on OpenShell 0.1.2 — see the comment above that
# call for why policy can't be applied as a separate, later step).
#
# Usage: ./15-provision-claude-sandbox.sh [--gw] <user-id> <server-name>[,<server-name>...]
#   e.g. ./15-provision-claude-sandbox.sh bob mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#   e.g. ./15-provision-claude-sandbox.sh alice mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance,mcp-compatibility
#   e.g. ./15-provision-claude-sandbox.sh --gw bob mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#
# --gw (bare flag, default off) routes this sandbox's MCP calls through the
# RHCL MCP Gateway (../mcp-gateway, which must already be deployed with
# auth.enabled) instead of directly to each server's own port-8000 Envoy
# listener. mcp-servers.json gets ONE "gateway" entry instead of one entry
# per server — every server's tools are still reachable, just through the
# gateway's single /mcp broker, surfaced as e.g.
# mcp__gateway__mcp_portfolio_list_my_clients instead of
# mcp__portfolio__list_my_clients (the broker prefixes tool names by
# server, see ../mcp-gateway/README.md). The policy's egress allow-list
# gets one allow_mcp_gateway group instead of one per server — same
# server-name list you'd otherwise pass, still validated the same way,
# just not used for per-server endpoint grants in this mode.
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

# Preserved across the `source .env` below: a caller that exported these
# deliberately (16-provision-audited-sandbox.sh sets CLAUDE_IMAGE to the
# *-audit image, and SANDBOX_PREFIX to "aud-") must win over .env's own
# values. `set -a; source .env` reassigns unconditionally, so without this
# the override is silently discarded. That failure is invisible in the worst
# way: SANDBOX_PREFIX isn't in .env so it survives, and you get a correctly
# named `aud-` sandbox running the plain image, no session-auditor binary
# in it, and a Scene 7 that answers prompts normally while never emitting a
# single audit metric.
_CALLER_CLAUDE_IMAGE="${CLAUDE_IMAGE-}"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

[ -n "$_CALLER_CLAUDE_IMAGE" ] && CLAUDE_IMAGE="$_CALLER_CLAUDE_IMAGE"

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
# Per-user LLM key, falling back to the shared one. ANTHROPIC_API_KEY_BOB
# wins over ANTHROPIC_API_KEY when provisioning bob.
#
# The provider is already per-workspace (`provider create ... --workspace
# "${USER_ID}"` below), so each banker has had their own credential slot
# all along -- only the value was shared. Giving each their own key is
# what makes upstream token accounting per-banker rather than one pooled
# bill, which matters once the endpoint meters per key (an OpenShift MaaS
# subscription does).
#
# Rotating a key later does NOT require re-provisioning: the sandbox env
# holds a resolve-placeholder, not the key, and the proxy substitutes the
# current value per request. Verified live --
#   openshell provider update byo-claude \
#     --credential "ANTHROPIC_AUTH_TOKEN=<new>" --workspace <user> --wait
# takes effect on a running sandbox (a bad key starts failing immediately,
# a good one recovers, with no sandbox touched).
USER_ID_UC=$(printf '%s' "$USER_ID" | tr '[:lower:]-' '[:upper:]_')
_USER_KEY_VAR="ANTHROPIC_API_KEY_${USER_ID_UC}"
ANTHROPIC_API_KEY="${!_USER_KEY_VAR:-${ANTHROPIC_API_KEY:-}}"
: "${ANTHROPIC_API_KEY:?set ${_USER_KEY_VAR} (preferred) or ANTHROPIC_API_KEY in .env}"
if [ -n "${!_USER_KEY_VAR:-}" ]; then
  echo "Using per-user LLM key from ${_USER_KEY_VAR}."
else
  echo "No ${_USER_KEY_VAR} set; falling back to the shared ANTHROPIC_API_KEY."
fi
: "${ANTHROPIC_BASE_URL:?set ANTHROPIC_BASE_URL in .env}"
: "${ANTHROPIC_MODEL:?set ANTHROPIC_MODEL in .env}"

IFS=',' read -ra SERVERS <<< "$SERVER_NAMES"
[ ${#SERVERS[@]} -gt 0 ] || { echo "no server names given"; exit 1; }

# In 0.0.106 the proxy only injects credentials for matching endpoints, so
# the provider profile's <llm-host>/<llm-port> placeholders must match the
# real host/port. Defaults to 443 (the historical assumption, an external
# HTTPS Route) if ANTHROPIC_BASE_URL doesn't specify a port — e.g. a plain
# http://<svc>.<ns>.svc.cluster.local:<port> in-cluster endpoint (bypassing
# an external Route's self-signed-cert forward-proxy path entirely, see
# that section of the README) needs its real, non-443 port here.
LLM_HOST_PORT=$(echo "$ANTHROPIC_BASE_URL" | sed 's|https\?://||;s|/.*||')
LLM_HOST="${LLM_HOST_PORT%%:*}"
if [[ "$LLM_HOST_PORT" == *:* ]]; then
  LLM_PORT="${LLM_HOST_PORT##*:}"
else
  LLM_PORT=443
fi

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
sed -e "s/<llm-host>/${LLM_HOST}/" -e "s/<llm-port>/${LLM_PORT}/" -e "s/<openshell-namespace>/${OPENSHELL_NAMESPACE}/g" providers/byo-claude-profile.yaml > "$TMPFILE"
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
  --credential "ANTHROPIC_AUTH_TOKEN=$ANTHROPIC_API_KEY" \
  --workspace "${USER_ID}" || true

# ---------------------------------------------------------------------------
# Step 2: build /sandbox/.claude/mcp-servers.json locally (one entry per
# server, keyed by its short name with the "mcp-" prefix stripped — Claude's
# tool names come out as mcp__<key>__<tool>, e.g.
# mcp__portfolio__list_my_clients — the URL still uses the real service DNS
# name) with a __USER_ACCESS_TOKEN__ placeholder. The real token isn't known
# yet — user-<id> (which injects it as $USER_ACCESS_TOKEN) is only attached
# once the sandbox exists — so this is still a placeholder when it's
# uploaded at creation time; step 4 substitutes it after.
# ---------------------------------------------------------------------------
MCP_CONFIG=$(mktemp --suffix=.json)
if [ "$GW" = true ]; then
  printf '{"mcpServers":{"gateway":{"type":"http","url":"%s/mcp","headers":{"Authorization":"Bearer __USER_ACCESS_TOKEN__"}}}}' \
    "$MCP_GATEWAY_URL" > "$MCP_CONFIG"
else
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
fi

# ---------------------------------------------------------------------------
# Step 3: render the full policy via the same Helm chart 14 uses for Codex
# (recipe=claude-code instead of recipe=codex) *before* creating the
# sandbox, so it can be passed to `sandbox create --policy` directly.
#
# This isn't just a reordering for tidiness: confirmed live on OpenShell
# 0.1.2, a sandbox created with --provider flags but no --policy has no
# usable policy at all (no built-in bundle the way 0.0.106 had) and sits in
# a "ConfigurationInvalid" rejected state — `sandbox create` then blocks
# client-side polling for a Ready state that can never arrive (anywhere
# from ~30s to its 5-minute timeout), while the gateway's own independent
# fail-closed safeguard deletes the still-rejected sandbox on its own clock
# in the meantime (`rolled back stale fail-closed sandbox-runtime
# bootstrap` in gateway logs) — a race between two uncoordinated timers
# that a policy-less `sandbox create` reliably loses. Passing a real
# --policy at creation time sidesteps the race entirely rather than trying
# to win it: the sandbox is never in the rejected state to begin with.
# Confirmed live, repeatedly: `sandbox create --policy ... --upload ...`
# (same call, no backgrounding/sleep tricks needed) returns normally in
# ~35s (ordinary image-pull time) and reaches Ready immediately.
# ---------------------------------------------------------------------------
POLICY_TMPFILE=$(mktemp --suffix=.yaml)
POLICY_TEMPLATE_ARGS=(
  --set openshellNamespace="${OPENSHELL_NAMESPACE}"
  --set llmHost="${LLM_HOST}"
  --set recipe=claude-code
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

SANDBOX_CREATE_ARGS=(
  --name "$SANDBOX_NAME"
  --provider byo-claude
  --provider "user-${USER_ID}"
  --policy "$POLICY_TMPFILE"
  --upload "${MCP_CONFIG}:/sandbox/.claude/mcp-servers.json"
  --workspace "${USER_ID}"
)
[ -z "$CLAUDE_IMAGE" ] || SANDBOX_CREATE_ARGS+=(--from "$CLAUDE_IMAGE")

# Note: if $SANDBOX_NAME already exists, this is a no-op — the --policy and
# --upload'd mcp-servers.json only take effect at creation time. Re-running
# with a changed server list against an already-provisioned sandbox won't
# update it; delete the sandbox first if you need to change its MCP
# servers or policy.
openshell sandbox create "${SANDBOX_CREATE_ARGS[@]}" --detach || true
rm -f "$POLICY_TMPFILE" "$MCP_CONFIG"

# The two `provider attach` calls right after are the fallback for that
# same "already existed" case: if $SANDBOX_NAME predates this script (e.g.
# created some other way with only one of the two providers attached),
# `sandbox create ... || true` above silently does nothing — it would
# never actually attach byo-claude/user-<id>, leaving Claude Code with no
# LLM credentials with no error raised. Attaching an already-attached
# provider is a harmless no-op, so these are safe to run unconditionally.
openshell sandbox provider attach "$SANDBOX_NAME" byo-claude --workspace "${USER_ID}" || true
openshell sandbox provider attach "$SANDBOX_NAME" "user-${USER_ID}" --workspace "${USER_ID}" || true

# ---------------------------------------------------------------------------
# Step 4: substitute the real $USER_ACCESS_TOKEN into the uploaded file.
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

if [ "$GW" = true ]; then
  echo "Claude sandbox $SANDBOX_NAME provisioned in workspace ${USER_ID}, routed through the MCP Gateway (${MCP_GATEWAY_URL}) to: ${SERVER_NAMES}"
else
  echo "Claude sandbox $SANDBOX_NAME provisioned in workspace ${USER_ID}, wired to: ${SERVER_NAMES}"
fi
openshell sandbox list --workspace "${USER_ID}"
