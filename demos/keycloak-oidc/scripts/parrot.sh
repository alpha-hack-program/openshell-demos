#!/usr/bin/env bash
set -eo pipefail
# Runs `parrot` manually against one sandbox/turn, threading through exactly
# the same --env wiring scripts/19-run-scene7-persona-demo.sh builds for its
# own turns (OTel, session-auditor, MLflow) — without script 19's own
# multi-scene narrative loop. Useful for a one-off prompt, a quick smoke
# test, or driving a sandbox/agent combination script 19 doesn't script for
# you (e.g. a fourth banker, or a scene not in script 19's own lists).
#
# `set -u` is deliberately NOT set here (unlike most scripts in this repo):
# bash 3.2 (macOS's system bash) throws "unbound variable" on a bare
# "${arr[@]}" expansion of a genuinely empty array even when explicitly
# declared with "=()" -- the same bug scripts/19 hit and fixed with the
# "${arr[@]+"${arr[@]}"}" guard. That guard is used below too, but omitting
# `set -u` here as well is the simpler, equally safe choice for a script
# this short.
#
# MUST run from an actual interactive terminal (a real pty) -- parrot
# always draws its dashboard and needs one; it will not work backgrounded,
# piped, or run non-interactively. See "Using parrot instead of raw CLI
# invocations" in the README for the general caveats (one-shot --prompt
# runs still wait for a keypress to close the dashboard, cache hits are
# weird across restarts, etc).
#
# Usage: ./parrot.sh [--agent claude|codex] [--aud] [--ui] <user-id> [-- <prompt text...>]
#   e.g. ./parrot.sh bob
#     -> interactive session against claude-bob
#   e.g. ./parrot.sh --agent codex --aud bob -- "What is 2+2?"
#     -> one-shot prompt against aud-codex-bob, with session-auditor's env
#   e.g. ./parrot.sh --ui --aud alice -- "Summarize today's meetings"
#     -> one-shot via parrot-tauri against aud-claude-alice, with MLflow
#        tracing env vars threaded through (both agents; harmless no-op
#        when MLFLOW_TRACKING_HOST is unset in .env)
#
# --agent claude|codex (default claude) picks which sandbox/CLI to drive.
# --aud (bare flag, default off) targets aud-<agent>-<user-id> instead of
#   the plain <agent>-<user-id> sandbox -- must already be provisioned
#   (scripts/16-/17-provision-*.sh for admin, or scripts/14-/15- for the
#   plain variant).
# --ui (bare flag, default off) uses parrot-tauri instead of parrot-tui.
# Everything after a literal `--` is passed to parrot as --prompt; omit it
# for an interactive multi-turn session instead of a one-shot run.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."
cd "$DEMO_DIR"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

AGENT="claude"
AUD=false
UI="parrot"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent)
      AGENT="$2"
      shift 2
      ;;
    --aud)
      AUD=true
      shift
      ;;
    --ui)
      UI="parrot-tauri"
      shift
      ;;
    --)
      shift
      break
      ;;
    -h|--help)
      sed -n '2,33p' "${BASH_SOURCE[0]}"
      exit 0
      ;;
    *)
      USER_ID="$1"
      shift
      ;;
  esac
done
: "${USER_ID:?usage: $0 [--agent claude|codex] [--aud] [--ui] <alice|bob|charlie> [-- <prompt text...>]}"
PROMPT="$*"

case "$AGENT" in
  claude|codex) ;;
  *) echo "unknown --agent '$AGENT' (expected 'claude' or 'codex')" >&2; exit 1 ;;
esac

if [ "$AUD" = true ]; then
  SANDBOX_NAME="aud-${AGENT}-${USER_ID}"
else
  SANDBOX_NAME="${AGENT}-${USER_ID}"
fi

# Always this user's own identity, regardless of what the shell had
# exported before -- same deliberate reset scripts/19/as.sh use.
export XDG_CONFIG_HOME="$HOME/.local/state/openshell-demos/oc-${USER_ID}/config"
export XDG_STATE_HOME="$HOME/.local/state/openshell-demos/oc-${USER_ID}/state"

GATEWAY_NAME="${GATEWAY_NAME:-openshift}"
GATEWAY_METADATA="$XDG_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/metadata.json"
if [[ ! -f "$GATEWAY_METADATA" ]]; then
  cat >&2 <<EOF
${USER_ID} has never logged into gateway '${GATEWAY_NAME}' from this
identity (nothing at $GATEWAY_METADATA). Do the browser login first (see
"Log in as each banker" in the README), then re-run this script.
EOF
  exit 1
fi

command -v "$UI" >/dev/null 2>&1 || {
  echo "$UI not found on PATH -- see the README's" \
       "'Using parrot instead of raw CLI invocations' for install, or point" \
       "PATH at util/parrot/target/debug/parrot (built from source)." >&2
  exit 1
}

# shellcheck source=lib-otel-env.sh
source "$SCRIPT_DIR/lib-otel-env.sh"
if [[ "$AGENT" == "claude" ]]; then
  otel_claude_env_args "$USER_ID" "$SANDBOX_NAME"
else
  otel_codex_env_args "$USER_ID" "$SANDBOX_NAME"
fi

# Harmless no-op against a non-audited sandbox (no session-auditor Stop
# hook there to read these).
# shellcheck source=lib-auditor-env.sh
source "$SCRIPT_DIR/lib-auditor-env.sh"
auditor_env_args

# MLflow tracing -- agent-agnostic (Claude's Python autolog hook and
# Codex's own in-process session-auditor trace push both read a subset of
# the same vars and ignore the rest, see lib-mlflow-env.sh) -- harmless
# empty array when MLFLOW_TRACKING_HOST is unset in .env.
# shellcheck source=lib-mlflow-env.sh
source "$SCRIPT_DIR/lib-mlflow-env.sh"
mlflow_env_args
MLFLOW_ARGS=("${MLFLOW_ENV_ARGS[@]+"${MLFLOW_ENV_ARGS[@]}"}")

if [[ "$UI" == "parrot" ]]; then
  echo "(press any key, e.g. q, to close the parrot dashboard once the turn finishes)"
else
  echo "(close the parrot-tauri window once the turn finishes)"
fi

PARROT_ARGS=(--sandbox "$SANDBOX_NAME" --workspace "$USER_ID" --agent "$AGENT")
PARROT_ARGS+=("${OTEL_ENV_ARGS[@]+"${OTEL_ENV_ARGS[@]}"}")
PARROT_ARGS+=("${AUDITOR_ENV_ARGS[@]+"${AUDITOR_ENV_ARGS[@]}"}")
PARROT_ARGS+=("${MLFLOW_ARGS[@]+"${MLFLOW_ARGS[@]}"}")
[ -z "$PROMPT" ] || PARROT_ARGS+=(--prompt "$PROMPT")

"$UI" "${PARROT_ARGS[@]}"
