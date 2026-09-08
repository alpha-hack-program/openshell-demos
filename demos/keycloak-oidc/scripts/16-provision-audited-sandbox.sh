#!/usr/bin/env bash
set -euo pipefail
# Provisions a session-audited counterpart of a banker's sandbox for
# Scene 7 ("Watching the audit trail live") — reuses
# 14-provision-codex-sandbox.sh / 15-provision-claude-sandbox.sh as-is
# (CODEX_IMAGE/CLAUDE_IMAGE pointed at the published *-audit image,
# SANDBOX_PREFIX=aud- so it lands alongside, not instead of, the banker's
# existing sandbox), then imports and attaches the session-auditor-*
# provider so the SessionStart/UserPromptSubmit/Stop hooks baked into that
# image can push heartbeat + compliance-risk metrics. See
# util/session-auditor/README.md and
# demos/keycloak-oidc/docs/prometheus-scraping.md for what this buys you,
# and demos/keycloak-oidc/audit-collector/README.md for the collector this
# pushes to (deploy that chart first).
#
# Usage: ./16-provision-audited-sandbox.sh <user-id> <claude|codex> <server-name>[,<server-name>...]
#   e.g. ./16-provision-audited-sandbox.sh bob claude mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#   e.g. ./16-provision-audited-sandbox.sh bob codex mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance
#
# Requires CLAUDE_AUDIT_IMAGE (for claude) or CODEX_AUDIT_IMAGE (for
# codex) set in .env — a published claude-audit/codex-audit image tag, see
# util/session-auditor/README.md's "Getting the binary" / "Releasing".
# Also requires whichever of ANTHROPIC_API_KEY/ANTHROPIC_MODEL (claude) or
# OPENAI_API_KEY/OPENAI_MODEL (codex) 14/15 already require — reused here
# as the classification credential too, just injected under the
# AUDITOR_-prefixed env names the session-auditor-* provider profile
# declares (never the bare ANTHROPIC_*/OPENAI_* names — confirmed live
# that attaching two providers which both claim the same credential env
# key fails at `sandbox provider attach` time, see
# util/session-auditor/README.md's "Classification backend" section).
#
# AUDITOR_PROVIDER_STYLE (optional, default "anthropic") selects between
# providers/session-auditor-anthropic-profile.yaml and
# session-auditor-openai-profile.yaml (see util/session-auditor/README.md
# "Classification backend: Anthropic or OpenAI-compatible").
#
# NOTE: both session-auditor-*-profile.yaml files hardcode their egress
# endpoint to api.deepseek.com, not a <llm-host> placeholder like
# byo-claude-profile.yaml — this script reuses ANTHROPIC_BASE_URL/
# OPENAI_BASE_URL as AUDITOR_LLM_BASE_URL's printed value on the assumption
# your .env's classification backend already is DeepSeek (matching this
# demo's own deepseek-claude precedent). If yours points somewhere else,
# either edit the profile's endpoint host before importing it, or attach a
# host that matches — network policy will otherwise deny the classify call.
#
# Idempotent, same conventions as 14/15 (provider/attach calls tolerate
# "already exists").

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

USER_ID="${1:?usage: $0 <user-id> <claude|codex> <server-name>[,<server-name>...]}"
AGENT="${2:?usage: $0 <user-id> <claude|codex> <server-name>[,<server-name>...]}"
SERVER_NAMES="${3:?usage: $0 <user-id> <claude|codex> <server-name>[,<server-name>...]}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"

AUDITOR_PROVIDER_STYLE="${AUDITOR_PROVIDER_STYLE:-anthropic}"

cd "$DEMO_DIR"

# ---------------------------------------------------------------------------
# Step 1: hand off to the existing per-agent script, pointed at the
# *-audit image and prefixed so it creates a second sandbox rather than
# touching the banker's existing one.
# ---------------------------------------------------------------------------
case "$AGENT" in
  claude)
    : "${CLAUDE_AUDIT_IMAGE:?set CLAUDE_AUDIT_IMAGE in .env (see util/session-auditor/README.md)}"
    SANDBOX_NAME="aud-claude-${USER_ID}"
    AUDITOR_CREDENTIAL_KEY="AUDITOR_ANTHROPIC_API_KEY"
    AUDITOR_MODEL_KEY="AUDITOR_ANTHROPIC_MODEL"
    CLAUDE_IMAGE="$CLAUDE_AUDIT_IMAGE" SANDBOX_PREFIX="aud-" \
      "$SCRIPT_DIR/15-provision-claude-sandbox.sh" "$USER_ID" "$SERVER_NAMES"
    if [ "$AUDITOR_PROVIDER_STYLE" = "anthropic" ]; then
      : "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY in .env}"
      : "${ANTHROPIC_MODEL:?set ANTHROPIC_MODEL in .env}"
      : "${ANTHROPIC_BASE_URL:?set ANTHROPIC_BASE_URL in .env}"
      AUDITOR_CREDENTIAL_VALUE="$ANTHROPIC_API_KEY"
      AUDITOR_MODEL_VALUE="$ANTHROPIC_MODEL"
      AUDITOR_BASE_URL_VALUE="$ANTHROPIC_BASE_URL"
    fi
    ;;
  codex)
    : "${CODEX_AUDIT_IMAGE:?set CODEX_AUDIT_IMAGE in .env (see util/session-auditor/README.md)}"
    SANDBOX_NAME="aud-codex-${USER_ID}"
    AUDITOR_CREDENTIAL_KEY="AUDITOR_ANTHROPIC_API_KEY"
    AUDITOR_MODEL_KEY="AUDITOR_ANTHROPIC_MODEL"
    CODEX_IMAGE="$CODEX_AUDIT_IMAGE" SANDBOX_PREFIX="aud-" \
      "$SCRIPT_DIR/14-provision-codex-sandbox.sh" "$USER_ID" "$SERVER_NAMES"
    if [ "$AUDITOR_PROVIDER_STYLE" = "anthropic" ]; then
      : "${ANTHROPIC_API_KEY:?set ANTHROPIC_API_KEY in .env (classification backend credential)}"
      : "${ANTHROPIC_MODEL:?set ANTHROPIC_MODEL in .env (classification backend model)}"
      : "${ANTHROPIC_BASE_URL:?set ANTHROPIC_BASE_URL in .env (classification backend URL)}"
      AUDITOR_CREDENTIAL_VALUE="$ANTHROPIC_API_KEY"
      AUDITOR_MODEL_VALUE="$ANTHROPIC_MODEL"
      AUDITOR_BASE_URL_VALUE="$ANTHROPIC_BASE_URL"
    fi
    ;;
  *)
    echo "unknown agent '$AGENT' — expected 'claude' or 'codex'" >&2
    exit 1
    ;;
esac

if [ "$AUDITOR_PROVIDER_STYLE" = "openai" ]; then
  : "${OPENAI_API_KEY:?set OPENAI_API_KEY in .env (classification backend credential)}"
  : "${OPENAI_MODEL:?set OPENAI_MODEL in .env (classification backend model)}"
  : "${OPENAI_BASE_URL:?set OPENAI_BASE_URL in .env (classification backend URL)}"
  AUDITOR_CREDENTIAL_KEY="AUDITOR_OPENAI_API_KEY"
  AUDITOR_MODEL_KEY="AUDITOR_OPENAI_MODEL"
  AUDITOR_CREDENTIAL_VALUE="$OPENAI_API_KEY"
  AUDITOR_MODEL_VALUE="$OPENAI_MODEL"
  AUDITOR_BASE_URL_VALUE="$OPENAI_BASE_URL"
fi

# ---------------------------------------------------------------------------
# Step 2: import + create the session-auditor-* provider (endpoints scoped
# to /usr/local/bin/session-auditor only — see the profile's own comments)
# and attach it alongside whatever 14/15 already attached.
# ---------------------------------------------------------------------------
openshell provider profile import \
  -f "providers/session-auditor-${AUDITOR_PROVIDER_STYLE}-profile.yaml" \
  --workspace "${USER_ID}" || true

openshell provider create --name "session-auditor-${AUDITOR_PROVIDER_STYLE}" \
  --type "session-auditor-${AUDITOR_PROVIDER_STYLE}" \
  --credential "${AUDITOR_CREDENTIAL_KEY}=${AUDITOR_CREDENTIAL_VALUE}" \
  --workspace "${USER_ID}" || true

openshell sandbox provider attach "$SANDBOX_NAME" "session-auditor-${AUDITOR_PROVIDER_STYLE}" \
  --workspace "${USER_ID}" || true

echo
echo "Audited $AGENT sandbox $SANDBOX_NAME provisioned in workspace ${USER_ID}, wired to: ${SERVER_NAMES}"
echo "session-auditor's SessionStart/UserPromptSubmit hooks need no further config."
echo "Its Stop hook (compliance classification) needs its own model set per turn, ON TOP of"
echo "whatever env vars the agent itself already needs (ANTHROPIC_*/OPENAI_*), e.g.:"
if [ "$AGENT" = "claude" ]; then
  echo "  openshell sandbox exec -n $SANDBOX_NAME --workspace ${USER_ID} \\"
  echo "    --env ANTHROPIC_BASE_URL=\$ANTHROPIC_BASE_URL --env ANTHROPIC_MODEL=\$ANTHROPIC_MODEL \\"
  echo "    --env AUDITOR_LLM_BASE_URL=${AUDITOR_BASE_URL_VALUE} --env ${AUDITOR_MODEL_KEY}=${AUDITOR_MODEL_VALUE} \\"
  echo "    -- claude --mcp-config /sandbox/.claude/mcp-servers.json --strict-mcp-config \\"
  echo "       -p \"...\" --permission-mode bypassPermissions --output-format text"
else
  echo "  openshell sandbox exec -n $SANDBOX_NAME --workspace ${USER_ID} \\"
  echo "    --env AUDITOR_LLM_BASE_URL=${AUDITOR_BASE_URL_VALUE} --env ${AUDITOR_MODEL_KEY}=${AUDITOR_MODEL_VALUE} \\"
  echo "    -- codex exec \"...\""
fi
openshell sandbox list --workspace "${USER_ID}"
