#!/usr/bin/env bash
set -euo pipefail
# Re-registers one identity's local openshell CLI session (admin or a
# banker) against whatever cluster THIS demo directory's own .env points
# at right now. Run this whenever `use_identity` (via `source scripts/as.sh
# <id>` or lib-use-identity.sh directly) reports a stale-cluster mismatch
# -- e.g. you've been running this same identity against a different
# cluster in another checkout/session, and the XDG identity directories
# under $HOME/.local/state/openshell-demos/oc-<id>/ (shared by every
# checkout on this machine, not scoped per-repo-path) still point there.
#
# This does NOT remove the one genuinely manual step in step 2b /
# "Log in as each banker": a real browser OIDC login as that identity.
# It automates everything around it -- pulling or copying the right mTLS
# material, clearing the stale gateway registration, and running
# `gateway add` so the browser opens pointed at the right endpoint.
#
# Usage: ./scripts/relogin-identity.sh <admin|alice|bob|charlie>
#
# admin: pulls fresh mTLS material straight from the cluster via
#   `oc get secret openshell-client-tls` (same as step 2b) -- requires a
#   live `oc` session against the target cluster.
# alice/bob/charlie: copies mTLS material from admin's own
#   already-registered identity for this cluster (same as "Log in as
#   each banker" -- the three mTLS files are identical for every identity
#   on one gateway, they authenticate the connection, not the user).
#   Requires admin's own identity to already be correctly registered
#   against the target cluster; run `./scripts/relogin-identity.sh admin`
#   first if it isn't.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."
cd "$DEMO_DIR"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

USER_ID="${1:?usage: $0 <admin|alice|bob|charlie>}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
KEYCLOAK_REALM="${KEYCLOAK_REALM:-openshell}"
KEYCLOAK_CLIENT_ID_CLI="${KEYCLOAK_CLIENT_ID_CLI:-openshell-cli}"
GATEWAY_NAME="${GATEWAY_NAME:-openshift}"
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"

export XDG_CONFIG_HOME="$HOME/.local/state/openshell-demos/oc-${USER_ID}/config"
export XDG_STATE_HOME="$HOME/.local/state/openshell-demos/oc-${USER_ID}/state"
mkdir -p "$XDG_CONFIG_HOME" "$XDG_STATE_HOME"

MTLS_DIR="$XDG_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/mtls"
mkdir -p "$MTLS_DIR"

if [[ "$USER_ID" == "admin" ]]; then
  echo "Pulling mTLS material from the cluster (namespace ${OPENSHELL_NAMESPACE})..."
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
  oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"
  # If using the Let's Encrypt path, the CLI's gRPC control channel pins
  # trust to this ca.crt and does NOT fall back to the system trust store
  # -- append the Route's issuing chain, same as step 2b.
  if [[ -n "${LETSENCRYPT_CLUSTER_ISSUER:-}" ]]; then
    echo | openssl s_client -connect "${ROUTE_HOST}:443" -servername "${ROUTE_HOST}" -showcerts 2>/dev/null \
      | awk '/-----BEGIN CERTIFICATE-----/{n++} n>=2' >> "$MTLS_DIR/ca.crt"
  fi
else
  ADMIN_CONFIG_HOME="$HOME/.local/state/openshell-demos/oc-admin/config"
  ADMIN_MTLS_DIR="$ADMIN_CONFIG_HOME/openshell/gateways/$GATEWAY_NAME/mtls"
  if [[ ! -f "$ADMIN_MTLS_DIR/ca.crt" ]]; then
    echo "admin has no registered mTLS material for gateway '$GATEWAY_NAME' at $ADMIN_MTLS_DIR." >&2
    echo "Run '$0 admin' first (against the current cluster), then retry for $USER_ID." >&2
    exit 1
  fi
  echo "Copying mTLS material from admin's identity..."
  cp "$ADMIN_MTLS_DIR/ca.crt" "$ADMIN_MTLS_DIR/tls.crt" "$ADMIN_MTLS_DIR/tls.key" "$MTLS_DIR/"
fi

openshell gateway remove "$GATEWAY_NAME" 2>/dev/null || true
echo "Registering gateway and opening a browser for ${USER_ID}'s OIDC login..."
openshell gateway add "https://${ROUTE_HOST}:443" \
  --name "$GATEWAY_NAME" \
  --oidc-issuer "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}" \
  --oidc-client-id "$KEYCLOAK_CLIENT_ID_CLI" \
  --oidc-scopes "openid offline_access"

openshell whoami   # confirm: Name matches $USER_ID, against the current cluster
