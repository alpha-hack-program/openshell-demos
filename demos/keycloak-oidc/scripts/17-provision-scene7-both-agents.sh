#!/usr/bin/env bash
set -euo pipefail
# Admin-run convenience wrapper for Scene 7 ("Watching the audit trail
# live") provisioning: runs 16-provision-audited-sandbox.sh once for
# claude and once for codex (or just one, see --agent below), for the
# same banker, wired to the same four servers step 5 already gives every
# banker. Provider and policy management stay Platform-Admin operations
# regardless of workspace (see 15-provision-claude-sandbox.sh's header) —
# this script always runs as admin's own identity (see below), never a
# banker's. Pair with 18-run-scene7-audit-demo.sh, which the banker runs
# themselves afterward.
#
# Always runs as admin — like 18/19 always force their own identity to
# the banker being acted on, this script derives and exports
# XDG_CONFIG_HOME/XDG_STATE_HOME to the
# $HOME/.local/state/openshell-demos/oc-admin/{config,state} convention
# ("How to follow this guide" in the README) regardless of what your shell
# already had exported, so you can't accidentally run this as a banker
# identity just because your terminal's env still has one exported from
# an earlier scene. If admin has never registered/logged into the gateway
# from this identity yet, this script does it inline — the same mTLS
# extraction + `gateway add` step 2b uses, which triggers a real
# browser-based OIDC login (log in as openshell-admin/openshell-admin).
# It also cross-checks an already-registered gateway's endpoint against
# this demo's own .env, so a stale registration left over from a
# different cluster is caught here too, not misread as some other error
# further down.
#
# --agent claude|codex provisions only that agent's audited sandbox; omit
# it to provision both, the original behavior.
#
# Usage: ./17-provision-scene7-both-agents.sh [--agent claude|codex] <user-id>
#   e.g. ./17-provision-scene7-both-agents.sh bob
#   e.g. ./17-provision-scene7-both-agents.sh --agent claude bob
#
# 16 in turn hands off to 15-provision-claude-sandbox.sh /
# 14-provision-codex-sandbox.sh (CLAUDE_IMAGE/CODEX_IMAGE pointed at the
# *-audit image, SANDBOX_PREFIX=aud-) — this script doesn't call 14/15
# directly because 16 also attaches the session-auditor-* provider that
# 14/15 know nothing about; skipping 16 would provision the sandboxes
# without the Stop hook that makes Scene 7's dashboard update at all.
# CLAUDE_AUDIT_IMAGE/CODEX_AUDIT_IMAGE (see util/session-auditor/README.md)
# are validated inside 16 itself, per agent branch — not repeated here.
# Idempotent — see 16's own header for what "tolerates already exists"
# covers.
#
# NOTE: the Codex path is still [VERIFY] per the README's Scene 7 note —
# only the Claude Code path has been confirmed live end to end so far.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."
cd "$DEMO_DIR"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

AGENT=""
USER_ID=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --agent)
      AGENT="${2:?--agent requires a value: claude or codex}"
      shift 2
      ;;
    *)
      USER_ID="$1"
      shift
      ;;
  esac
done
: "${USER_ID:?usage: $0 [--agent claude|codex] <user-id>}"
case "$AGENT" in
  ""|claude|codex) ;;
  *) echo "invalid --agent '$AGENT' (expected claude or codex)" >&2; exit 1 ;;
esac

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${KEYCLOAK_CLIENT_ID_CLI:?set KEYCLOAK_CLIENT_ID_CLI in .env}"

# Always admin's own identity, regardless of what your shell had exported
# before — see the header note above for why this is deliberate, not just
# a convenience default.
export XDG_CONFIG_HOME="$HOME/.local/state/openshell-demos/oc-admin/config"
export XDG_STATE_HOME="$HOME/.local/state/openshell-demos/oc-admin/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"

GATEWAY_NAME="${GATEWAY_NAME:-openshift}"
GATEWAY_METADATA="$XDG_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/metadata.json"
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"

if [[ ! -f "$GATEWAY_METADATA" ]]; then
  echo "admin has never logged into gateway '${GATEWAY_NAME}' from this identity — registering now (see step 2b in the README)."

  # Same mTLS extraction step 2b uses — identical for every identity on
  # this gateway (it authenticates the connection, not the user).
  MTLS_DIR="$XDG_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/mtls"
  mkdir -p "$MTLS_DIR"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
    -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
    -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
    -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"

  # Let's Encrypt path: the gRPC control channel pins trust to this ca.crt
  # and doesn't fall back to the system trust store — append the issuing
  # chain, same as step 2b.
  if [[ -n "${LETSENCRYPT_CLUSTER_ISSUER:-}" ]]; then
    echo | openssl s_client -connect "${ROUTE_HOST}:443" -servername "${ROUTE_HOST}" -showcerts 2>/dev/null \
      | awk '/-----BEGIN CERTIFICATE-----/{n++} n>=2' >> "$MTLS_DIR/ca.crt"
  fi

  openshell gateway remove "$GATEWAY_NAME" 2>/dev/null || true
  # Triggers the browser-based OIDC login (log in as
  # openshell-admin/openshell-admin) — no separate `gateway login` needed.
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

SERVERS="mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance"

if [[ "$AGENT" != "codex" ]]; then
  "$SCRIPT_DIR/16-provision-audited-sandbox.sh" "$USER_ID" claude "$SERVERS"
fi
if [[ "$AGENT" != "claude" ]]; then
  "$SCRIPT_DIR/16-provision-audited-sandbox.sh" "$USER_ID" codex "$SERVERS"
fi

echo
case "$AGENT" in
  claude) echo "aud-claude-${USER_ID} provisioned in workspace ${USER_ID}." ;;
  codex)  echo "aud-codex-${USER_ID} provisioned in workspace ${USER_ID}." ;;
  *)      echo "aud-claude-${USER_ID} and aud-codex-${USER_ID} provisioned in workspace ${USER_ID}." ;;
esac
echo "Next: as ${USER_ID} (not admin), run ./18-run-scene7-audit-demo.sh ${USER_ID}"
