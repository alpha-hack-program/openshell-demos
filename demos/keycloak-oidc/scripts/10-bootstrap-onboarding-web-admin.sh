#!/usr/bin/env bash
set -euo pipefail
# One-time bootstrap of onboarding-web's own standing Platform-Admin
# `openshell` session — the credential the backend uses to run
# `provider refresh configure`/`refresh rotate` on behalf of whoever logs
# in through the web app. See
# demos/keycloak-oidc/docs/self-service-onboarding.md, decision #2.
#
# This does NOT create a Keycloak user or client — those are declared
# directly in keycloak/realm-export.json (the `openshell-onboarding-web`
# client and the `openshell-onboarding-svc` user), same as every other demo
# identity, and already exist once step 1c has imported the realm.
#
# What this script does need a real human for: `openshell gateway add`
# triggers a genuine browser-based OIDC login — there is no scripted
# shortcut for the CLI's own session (unlike the *user*-token acquisition
# in 03-onboard-user.sh, which can use a password-grant shortcut in a
# fully-controlled demo environment; doing the same for this service
# identity would be no more defensible than for a real user and isn't
# worth the shortcut for a one-time step). When the browser opens, log in
# as `openshell-onboarding-svc` — NOT the human `openshell-admin` account —
# so this identity stays separately revocable/auditable from the demo
# operator's own login.
#
# Usage: ./10-bootstrap-onboarding-web-admin.sh [output-dir]
# Produces a directory (default: ./onboarding-web-admin-session, gitignored)
# containing the XDG_CONFIG_HOME/XDG_STATE_HOME tree to package into the
# `onboarding-web-admin-session` Secret consumed by
# demos/keycloak-oidc/onboarding-web/'s Helm chart.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

# Source root .env for CLUSTER_APPS_DOMAIN
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
# Same formula as the gateway Route host computed inline in step 2a — not
# stored in .env, so derive it here rather than requiring the caller to
# export it. Override by exporting ROUTE_HOST yourself beforehand.
: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in the root .env}"
ROUTE_HOST="${ROUTE_HOST:-openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}}"

OUTPUT_DIR="${1:-$DEMO_DIR/onboarding-web-admin-session}"
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

echo "=== Logging in as the onboarding-web service identity ==="
echo "A browser window will open. Log in as 'openshell-onboarding-svc',"
echo "NOT the human admin account."
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
echo "Confirm the name above is 'openshell-onboarding-svc' — if it shows"
echo "something else, remove this gateway registration and re-run this"
echo "script, logging in as the correct identity this time."
echo
echo "Session material written under: $OUTPUT_DIR"

# A Secret's data keys are flat — `--from-file=key=<a directory>` is
# rejected by oc/kubectl once a key name is given. Tar the whole
# config+state tree into a single blob instead; the onboarding-web chart's
# initContainer un-tars it into an emptyDir before the app container starts
# (see demos/keycloak-oidc/onboarding-web/templates/deployment.yaml).
TARBALL="$OUTPUT_DIR/admin-session.tar.gz"
tar czf "$TARBALL" -C "$OUTPUT_DIR" config state

echo "Packaged session material into: $TARBALL"
echo "Create the Secret the onboarding-web Deployment mounts with:"
echo
echo "  oc -n \"\$OPENSHELL_NAMESPACE\" create secret generic onboarding-web-admin-session \\"
echo "    --from-file=admin-session.tar.gz=\"$TARBALL\""
echo
echo "Then deploy with scripts/11-deploy-onboarding-web.sh."
