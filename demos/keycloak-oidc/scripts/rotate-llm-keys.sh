#!/usr/bin/env bash
set -euo pipefail
# Rotates the LLM API key(s) and/or the MLflow tracking token already baked
# into every banker's existing sandbox provider(s), without touching the
# sandbox itself. This is NOT a recreate script — recreating a sandbox
# (delete + re-run 14/15/16) is reserved for unrelated problems (stale
# trust roots, a changed provider-profile endpoint/policy); ordinary key
# rotation never needs it. `provider update` replaces the credential behind
# the sandbox's resolve-placeholder on a running sandbox — confirmed live,
# see README.md's "Rotating a key doesn't require re-provisioning" and
# docs/mlflow-tracing.md's "Minting and refreshing the token". Use
# `provider update`, never `provider profile update` — the latter
# regenerates the placeholder and breaks the mcp-servers.json already baked
# into the sandbox (403 "credential placeholder in the request body"),
# whose only fix is re-provisioning the sandbox — exactly what this script
# exists to avoid needing.
#
# Reads whatever is currently in .env — ANTHROPIC_API_KEY[_<USER>],
# CODEX_API_KEY[_<USER>] (falls back to OPENAI_API_KEY, same as
# 14-provision-codex-sandbox.sh), and MLFLOW_TRACKING_TOKEN — and pushes
# those values out to every already-provisioned banker's byo-claude,
# byo-codex, and mlflow providers. It does NOT mint a new MLflow token
# itself; run `oc create token openshell-sandbox -n "$OPENSHELL_NAMESPACE"
# --duration=8760h` and update MLFLOW_TRACKING_TOKEN in .env first (see
# docs/mlflow-tracing.md).
#
# A banker who was never provisioned for a given agent (no byo-claude/
# byo-codex/mlflow provider in their workspace) fails that one
# provider-update call with a clear per-banker/per-provider message, and
# the script continues to the rest — a missing provider isn't treated as
# "already done" the way create scripts treat "already exists", since here
# it usually means a real gap worth surfacing, not silently swallowing.
#
# Run as admin — `provider update` is a Platform-Admin/Workspace-Admin
# operation regardless of workspace, same as `provider create` (see
# 15-provision-claude-sandbox.sh's "Who runs this"). Every `provider
# update` call below fails identically with "The caller does not have
# permission to execute the specified operation" if the CLI identity
# running this script isn't admin -- rather than let that happen silently
# depending on whatever identity your shell last had active, this script
# always forces admin's own identity first (same XDG_CONFIG_HOME/
# XDG_STATE_HOME override and gateway-registration bootstrap as
# 17-provision-scene7-both-agents.sh), registering against the gateway
# with a real browser-based OIDC login if admin has never logged in from
# this identity before.
#
# Usage: ./rotate-llm-keys.sh [--agent claude|codex|both] [--dry-run] [--timeout SECONDS] [user-id ...]
#   e.g. ./rotate-llm-keys.sh                       # all agents, alice/bob/charlie
#   e.g. ./rotate-llm-keys.sh --agent claude         # only byo-claude + mlflow
#   e.g. ./rotate-llm-keys.sh --dry-run bob          # preview, bob only
#   e.g. ./rotate-llm-keys.sh --timeout 120          # slower cluster / more sandboxes per provider
#
# --agent selects which LLM credential to rotate (claude -> byo-claude /
# ANTHROPIC_AUTH_TOKEN, codex -> byo-codex / OPENAI_API_KEY; both is the
# default). The mlflow provider is always included when MLFLOW_TRACKING_HOST
# is set in .env, regardless of --agent, since it's shared by both agents.
# --dry-run prints the provider-update commands it would run instead of
# running them.
# Trailing positional args override the default banker roster
# (alice bob charlie) with a specific subset.
#
# Each rotation line also prints the base_url/model (or, for mlflow, the
# tracking_uri) currently in .env for that provider -- display only, this
# script never sets them: OpenShell only injects credentials via
# `provider update`, base_url/model are non-secret config passed via
# --env at `sandbox exec` time instead (see
# 15-provision-claude-sandbox.sh). This is just so you can see which
# endpoint/model a rotated key is currently paired with.
#
# --timeout N (default 60, vs. `provider update --wait`'s own CLI default
# of 30) raises the readiness-wait budget. That budget is SHARED across
# every sandbox attached to the provider being rotated, not per-sandbox --
# confirmed live: with mlflow attached to three of one banker's sandboxes,
# two converged well inside 30s while the third reported "superseded
# (desired_state_changed)" / "Wait: terminal" purely because the shared
# window ran out on it specifically, even though `openshell sandbox
# provider status <sandbox> mlflow --workspace <user>` showed it reach
# `ready` a couple of seconds later on its own. That's not a real
# rotation failure -- raise --timeout rather than retrying blindly.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."
cd "$DEMO_DIR"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

AGENT="both"
DRY_RUN=false
TIMEOUT=60
POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent)
      AGENT="${2:?--agent requires a value: claude, codex, or both}"
      shift 2
      ;;
    --dry-run)
      DRY_RUN=true
      shift
      ;;
    --timeout)
      TIMEOUT="${2:?--timeout requires a value in seconds}"
      shift 2
      ;;
    -h|--help)
      echo "Usage: $0 [--agent claude|codex|both] [--dry-run] [--timeout SECONDS] [user-id ...]"
      exit 0
      ;;
    *)
      POSITIONAL+=("$1")
      shift
      ;;
  esac
done
case "$AGENT" in
  claude|codex|both) ;;
  *) echo "invalid --agent '$AGENT' (expected claude, codex, or both)" >&2; exit 1 ;;
esac

# "${POSITIONAL[@]+...}" (not bare "${POSITIONAL[@]}") avoids "unbound
# variable" under `set -u` when POSITIONAL is empty on bash 3.2 (macOS's
# default) — same guard used in 15-provision-claude-sandbox.sh.
if [ ${#POSITIONAL[@]} -gt 0 ]; then
  BANKERS=("${POSITIONAL[@]}")
else
  BANKERS=("alice" "bob" "charlie")
fi

# ---------------------------------------------------------------------------
# Always admin's own identity, regardless of what your shell had exported
# before -- see the header note above for why. Identical to
# 17-provision-scene7-both-agents.sh's own bootstrap block.
# ---------------------------------------------------------------------------
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${KEYCLOAK_CLIENT_ID_CLI:?set KEYCLOAK_CLIENT_ID_CLI in .env}"

export XDG_CONFIG_HOME="$HOME/.local/state/openshell-demos/oc-admin/config"
export XDG_STATE_HOME="$HOME/.local/state/openshell-demos/oc-admin/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"

GATEWAY_NAME="${GATEWAY_NAME:-openshift}"
GATEWAY_METADATA="$XDG_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/metadata.json"
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"

if [[ ! -f "$GATEWAY_METADATA" ]]; then
  echo "admin has never logged into gateway '${GATEWAY_NAME}' from this identity — registering now (see step 2b in the README)."

  MTLS_DIR="$XDG_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/mtls"
  mkdir -p "$MTLS_DIR"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
    -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
    -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
    -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"
  # Shell redirection creates the file at 0644 under the usual 022 umask,
  # leaving a client private key readable by every other account on the
  # machine. Nothing fails as a result, which is why it goes unnoticed.
  chmod 600 "$MTLS_DIR/tls.key"

  if [[ -n "${LETSENCRYPT_CLUSTER_ISSUER:-}" ]]; then
    echo | openssl s_client -connect "${ROUTE_HOST}:443" -servername "${ROUTE_HOST}" -showcerts 2>/dev/null \
      | awk '/-----BEGIN CERTIFICATE-----/{n++} n>=2' >> "$MTLS_DIR/ca.crt"
  fi

  openshell gateway remove "$GATEWAY_NAME" 2>/dev/null || true
  openshell gateway add "https://${ROUTE_HOST}:443" \
    --name "$GATEWAY_NAME" \
    --oidc-issuer "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}" \
    --oidc-client-id "$KEYCLOAK_CLIENT_ID_CLI" \
    --oidc-scopes "openid offline_access"
fi

EXPECTED_ENDPOINT="https://${ROUTE_HOST}:443"
REGISTERED_ENDPOINT="$(jq -r '.gateway_endpoint' "$GATEWAY_METADATA" 2>/dev/null || true)"
if [[ "$REGISTERED_ENDPOINT" != "$EXPECTED_ENDPOINT" ]]; then
  cat >&2 <<EOF
admin's stored gateway registration points at a DIFFERENT cluster than
this demo's own .env expects:
  registered: ${REGISTERED_ENDPOINT:-<unreadable>}
  expected:   ${EXPECTED_ENDPOINT}
This is a stale login from a previous cluster/run, at
$GATEWAY_METADATA — remove it (openshell gateway remove "$GATEWAY_NAME")
and re-run this script to register fresh against the current cluster.
EOF
  exit 1
fi

openshell whoami   # confirm: Name: openshell-admin

FAILED=false

run_update() {
  local provider="$1" credential="$2" user="$3" context="${4:-}"
  local context_suffix=""
  [ -n "$context" ] && context_suffix=" (${context})"
  if [ "$DRY_RUN" = true ]; then
    echo "[dry-run] openshell provider update ${provider} --credential '${credential}' --workspace ${user} --wait --timeout ${TIMEOUT}${context_suffix}"
    return 0
  fi
  if openshell provider update "$provider" --credential "$credential" --workspace "$user" --wait --timeout "$TIMEOUT"; then
    echo "  ${provider} rotated for ${user}${context_suffix}."
  else
    echo "  FAILED: ${provider} for ${user} (not provisioned for this banker, or the update was rejected — see output above)." >&2
    FAILED=true
  fi
}

for USER_ID in "${BANKERS[@]}"; do
  echo "== ${USER_ID} =="
  USER_ID_UC=$(printf '%s' "$USER_ID" | tr '[:lower:]-' '[:upper:]_')

  if [[ "$AGENT" == "claude" || "$AGENT" == "both" ]]; then
    _ANTHROPIC_KEY_VAR="ANTHROPIC_API_KEY_${USER_ID_UC}"
    ANTHROPIC_KEY="${!_ANTHROPIC_KEY_VAR:-${ANTHROPIC_API_KEY:-}}"
    if [ -z "$ANTHROPIC_KEY" ]; then
      echo "  skipping byo-claude: neither ${_ANTHROPIC_KEY_VAR} nor ANTHROPIC_API_KEY set in .env." >&2
    else
      # base_url/model are non-secret config -- this call does NOT set
      # them (OpenShell only injects credentials; base_url/model are
      # passed via --env at `sandbox exec` time, see
      # 15-provision-claude-sandbox.sh). Shown here only so you can see
      # which endpoint/model this rotated key is currently paired with in
      # .env.
      run_update byo-claude "ANTHROPIC_AUTH_TOKEN=${ANTHROPIC_KEY}" "$USER_ID" \
        "base_url=${ANTHROPIC_BASE_URL:-<unset>}, model=${ANTHROPIC_MODEL:-<unset>}"
    fi
  fi

  if [[ "$AGENT" == "codex" || "$AGENT" == "both" ]]; then
    _CODEX_KEY_VAR="CODEX_API_KEY_${USER_ID_UC}"
    CODEX_KEY="${!_CODEX_KEY_VAR:-${CODEX_API_KEY:-${OPENAI_API_KEY:-}}}"
    if [ -z "$CODEX_KEY" ]; then
      echo "  skipping byo-codex: none of ${_CODEX_KEY_VAR}, CODEX_API_KEY, OPENAI_API_KEY set in .env." >&2
    else
      # Same CODEX_* -> OPENAI_* fallback as 14-provision-codex-sandbox.sh,
      # display-only (see note above).
      CODEX_DISPLAY_BASE_URL="${CODEX_BASE_URL:-${OPENAI_BASE_URL:-}}"
      CODEX_DISPLAY_MODEL="${CODEX_MODEL:-${OPENAI_MODEL:-}}"
      run_update byo-codex "OPENAI_API_KEY=${CODEX_KEY}" "$USER_ID" \
        "base_url=${CODEX_DISPLAY_BASE_URL:-<unset>}, model=${CODEX_DISPLAY_MODEL:-<unset>}"
    fi
  fi

  if [ -n "${MLFLOW_TRACKING_HOST:-}" ]; then
    if [ -z "${MLFLOW_TRACKING_TOKEN:-}" ]; then
      echo "  skipping mlflow: MLFLOW_TRACKING_HOST is set but MLFLOW_TRACKING_TOKEN is empty in .env (see docs/mlflow-tracing.md)." >&2
    else
      # Single shared token value across all bankers -- same for every
      # workspace, since the mlflow-sandbox-rbac grant is namespace-wide
      # (see docs/mlflow-tracing.md's "Minting and refreshing the token").
      # No "model" concept for mlflow (it's a tracking server, not an LLM
      # endpoint) -- tracking_uri built the same way as lib-mlflow-env.sh's
      # mlflow_env_args, display-only (see note above).
      MLFLOW_DISPLAY_URI="https://${MLFLOW_TRACKING_HOST}:${MLFLOW_TRACKING_PORT:-8443}/mlflow"
      run_update mlflow "MLFLOW_TRACKING_TOKEN=${MLFLOW_TRACKING_TOKEN}" "$USER_ID" \
        "tracking_uri=${MLFLOW_DISPLAY_URI}"
    fi
  fi
done

echo
if [ "$DRY_RUN" = true ]; then
  echo "Dry run complete -- no changes made."
elif [ "$FAILED" = true ]; then
  echo "Done, with at least one failure above." >&2
  exit 1
else
  echo "All requested credentials rotated."
fi
