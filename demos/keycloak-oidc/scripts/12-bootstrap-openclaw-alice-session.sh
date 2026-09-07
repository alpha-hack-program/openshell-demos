#!/usr/bin/env bash
set -euo pipefail
# One-time bootstrap of a standing `openshell` CLI session for alice, for
# the OpenClaw chat frontend (../openclaw/) to use as its sandbox-execution
# identity. Near-identical to 10-bootstrap-onboarding-web-admin.sh — same
# mTLS + OIDC session tarball mechanism — but logs in as an existing demo
# banker instead of the openshell-onboarding-svc service identity, since
# OpenClaw's OpenShell plugin has no auth of its own: it shells out to
# whatever `openshell` CLI session already exists for the OS user running
# the OpenClaw Gateway process (see ../openclaw/README.md).
#
# Prerequisite: alice must already be fully onboarded — workspace, provider,
# sandbox, MCP config (README.md steps 3.0/3a/4/5) — before this runs. This
# script does not provision any of that; it only captures a working CLI
# session for an identity that already works.
#
# Usage: ./12-bootstrap-openclaw-alice-session.sh [output-dir]
# Produces a directory (default: ../openclaw-alice-session, gitignored)
# containing the XDG_CONFIG_HOME/XDG_STATE_HOME tree to package into the
# `openclaw-alice-session` Secret consumed by ../openclaw's Helm chart.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

ROOT_ENV="$SCRIPT_DIR/../../../.env"
if [[ -f "$ROOT_ENV" ]]; then
  set -a; source "$ROOT_ENV"; set +a
fi

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${KEYCLOAK_HOST:?set in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${KEYCLOAK_CLIENT_ID_CLI:=openshell-cli}"
: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in the root .env}"
ROUTE_HOST="${ROUTE_HOST:-openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}}"

USER_ID="${1:-alice}"
OUTPUT_DIR="${2:-$DEMO_DIR/openclaw-${USER_ID}-session}"
mkdir -p "$OUTPUT_DIR/config" "$OUTPUT_DIR/state"

export XDG_CONFIG_HOME="$OUTPUT_DIR/config"
export XDG_STATE_HOME="$OUTPUT_DIR/state"

GATEWAY_NAME="${GATEWAY_NAME:-openshift}"
MTLS_DIR="$XDG_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/mtls"
mkdir -p "$MTLS_DIR"

oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"

if [[ -n "${LETSENCRYPT_CLUSTER_ISSUER:-}" ]]; then
  echo | openssl s_client -connect "${ROUTE_HOST}:443" -servername "${ROUTE_HOST}" -showcerts 2>/dev/null \
    | awk '/-----BEGIN CERTIFICATE-----/{n++} n>=2' >> "$MTLS_DIR/ca.crt"
fi

echo "=== Logging in as ${USER_ID} ==="
echo "A browser window will open. Log in as '${USER_ID}' — this must be the"
echo "same banker account already onboarded via README.md steps 3.0/3a/4/5."
echo

openshell gateway remove "$GATEWAY_NAME" 2>/dev/null || true
openshell gateway add "https://${ROUTE_HOST}:443" \
  --name "$GATEWAY_NAME" \
  --oidc-issuer "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}" \
  --oidc-client-id "$KEYCLOAK_CLIENT_ID_CLI" \
  --oidc-scopes "openid offline_access"

echo
echo "=== Confirming identity ==="
openshell whoami
echo
echo "Confirm the name above is '${USER_ID}' — if it shows something else,"
echo "remove this gateway registration and re-run this script, logging in"
echo "as the correct identity this time."
echo
echo "Session material written under: $OUTPUT_DIR"

# Same reasoning as 10-bootstrap-onboarding-web-admin.sh: a Secret's data
# keys are flat, so the multi-file config/state tree is packaged as a
# single tarball, unpacked by ../openclaw's initContainer into a real
# directory tree before the OpenClaw Gateway process starts.
TARBALL="$OUTPUT_DIR/${USER_ID}-session.tar.gz"
tar czf "$TARBALL" -C "$OUTPUT_DIR" config state

echo "Packaged session material into: $TARBALL"
echo "Create the Secret the openclaw Deployment mounts with:"
echo
echo "  oc -n \"\$OPENSHELL_NAMESPACE\" create secret generic openclaw-${USER_ID}-session \\"
echo "    --from-file=${USER_ID}-session.tar.gz=\"$TARBALL\""
echo
echo "Then create the LLM/gateway-token and oauth2-proxy Secrets described in"
echo "../openclaw/README.md, and deploy with scripts/13-deploy-openclaw.sh."
