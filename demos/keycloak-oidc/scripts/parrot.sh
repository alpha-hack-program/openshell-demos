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
# Usage: ./parrot.sh [--agent claude|codex] [--aud] [--ui] [--debug] <user-id> [-- <prompt text...>]
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
#   Its left-sidebar MCP Gateway toolbar/Today-rail/playground only appear
#   when --mcp-gateway-url/--keycloak-host are given -- this script infers
#   both from the user's already-stored OpenShell gateway metadata (no
#   `oc` call, no .env dependency), falling back to chat-only with a
#   warning if that metadata doesn't fit the expected shape.
# --debug (bare flag, default off, implies --ui) runs parrot-tauri via
#   `cargo tauri dev --no-watch` instead of the installed binary on PATH.
#   tauri-cli's built-in dev server serves util/parrot/parrot-tauri/ui/
#   straight off disk instead of the compiled-in copy `cargo build` embeds,
#   so a plain webview reload (right-click the window -> Reload; dev builds
#   run with the context menu/devtools enabled) picks up CSS/HTML/JS edits
#   instantly, with no rebuild. --no-watch turns OFF tauri-cli's own file
#   watcher on purpose: left on, it doesn't patch the page in place -- ANY
#   change under this directory (ui/ included) kills and relaunches the
#   whole process, re-running main()'s gateway connect and dropping your
#   session, which is worse than just reloading by hand. That also means
#   .rs changes need a manual quit + rerun of this command, not just a
#   reload. First run compiles the whole tauri/wry dependency stack from
#   scratch (several minutes); after that, rebuilds are incremental and
#   debug-profile (faster than --release). Requires `cargo` and tauri-cli
#   (`cargo install tauri-cli`) on PATH; does not need parrot-tauri
#   pre-installed anywhere.
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
DEBUG=false
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
    --debug)
      UI="parrot-tauri"
      DEBUG=true
      shift
      ;;
    --)
      shift
      break
      ;;
    -h|--help)
      sed -n '2,64p' "${BASH_SOURCE[0]}"
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

if [ "$DEBUG" = true ]; then
  command -v cargo >/dev/null 2>&1 || {
    echo "cargo not found on PATH -- --debug needs it (and tauri-cli," \
         "'cargo install tauri-cli') to run 'cargo tauri dev'." >&2
    exit 1
  }
  command -v cargo-tauri >/dev/null 2>&1 || {
    echo "tauri-cli not found on PATH -- --debug needs it: run" \
         "'cargo install tauri-cli'." >&2
    exit 1
  }
  PARROT_TAURI_DIR="$DEMO_DIR/../../util/parrot/parrot-tauri"
else
  command -v "$UI" >/dev/null 2>&1 || {
    echo "$UI not found on PATH -- see the README's" \
         "'Using parrot instead of raw CLI invocations' for install, or point" \
         "PATH at util/parrot/target/debug/parrot (built from source)." >&2
    exit 1
  }
fi

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

# parrot-tauri's left-sidebar toolbar/Today-rail/playground only appear
# when these are given -- parrot (TUI) has no such thing to configure.
# Inferred entirely from $GATEWAY_METADATA (already required to exist, see
# the check above) -- no `oc` call, no dependency on .env's
# CLUSTER_APPS_DOMAIN/KEYCLOAK_HOST being set. The inference leans on two
# facts that are conventions elsewhere in this repo, not guarantees:
#   - the gateway Route is always "openshell-<namespace>.<apps-domain>"
#     (see AGENTS.md's namespace-naming note), so stripping that prefix
#     recovers the apps domain.
#   - the MCP Gateway's own public host is "mcp.<apps-domain>" (same
#     convention mcp-inspector.sh/access_token.sh already hardcode).
# A gateway that doesn't follow either falls back to chat-only below,
# rather than guessing wrong.
if [[ "$UI" == "parrot-tauri" ]]; then
  GATEWAY_ENDPOINT_HOST=$(jq -r '.gateway_endpoint' "$GATEWAY_METADATA" | sed -E 's#^[a-z]+://##; s#:[0-9]+$##')
  OIDC_ISSUER=$(jq -r '.oidc_issuer // empty' "$GATEWAY_METADATA")
  if [[ "$GATEWAY_ENDPOINT_HOST" == openshell-*.* && -n "$OIDC_ISSUER" ]]; then
    CLUSTER_APPS_DOMAIN="${GATEWAY_ENDPOINT_HOST#openshell-*.}"
    MCP_GATEWAY_URL="https://mcp.${CLUSTER_APPS_DOMAIN}/mcp"
    KEYCLOAK_HOST_FROM_GATEWAY=$(echo "$OIDC_ISSUER" | sed -E 's#^https?://([^/]+)/.*#\1#')
    echo "parrot.sh: MCP Gateway = ${MCP_GATEWAY_URL}, Keycloak host = ${KEYCLOAK_HOST_FROM_GATEWAY} (inferred from '${GATEWAY_NAME}'s stored gateway metadata)" >&2
    PARROT_ARGS+=(--mcp-gateway-url "$MCP_GATEWAY_URL" --keycloak-host "$KEYCLOAK_HOST_FROM_GATEWAY")
  else
    echo "parrot.sh: could not infer the MCP Gateway URL/Keycloak host from '${GATEWAY_NAME}'s metadata ($GATEWAY_METADATA) -- launching chat-only, without the toolbar/Today rail" >&2
  fi
fi

[ -z "$PROMPT" ] || PARROT_ARGS+=(--prompt "$PROMPT")

if [ "$DEBUG" = true ]; then
  echo "parrot.sh: --debug -- running parrot-tauri via 'cargo tauri dev --no-watch'." \
       "Edit anything under util/parrot/parrot-tauri/ui/, then right-click the" \
       "window and choose Reload (WKWebView's native context menu -- Tauri dev" \
       "builds run with devtools/context-menu enabled) to see it: the files" \
       "are served straight off disk, so a reload alone picks up the change" \
       "instantly, with no rebuild and no reconnect to the sandbox. --no-watch" \
       "is deliberate: tauri-cli's file watcher doesn't patch the page in" \
       "place -- on ANY change under this directory (including ui/) it kills" \
       "and relaunches the whole process, which re-runs main()'s gateway" \
       "connect from scratch and drops your session. Changed a .rs file?" \
       "Quit the window and rerun this command instead of waiting for a" \
       "reload to pick it up. First run compiles the whole tauri/wry stack" \
       "from scratch, which takes a few minutes." >&2
  ( cd "$PARROT_TAURI_DIR" && cargo tauri dev --no-watch -- -- "${PARROT_ARGS[@]}" )
else
  "$UI" "${PARROT_ARGS[@]}"
fi
