#!/usr/bin/env bash
set -euo pipefail
# Deploys the onboarding-web chart. Prerequisites, both one-time and both
# needing a real cluster/Keycloak admin session, not scripted here:
#   1. The `openshell-onboarding-web` client (public, PKCE-required — see
#      the comment on that client in keycloak/realm-export.json for why
#      it must be public, not confidential) and `openshell-onboarding-svc`
#      user must already exist (realm-export.json, imported in step 1c).
#   2. scripts/10-bootstrap-onboarding-web-admin.sh must have already run
#      and its output Secret (onboarding-web-admin-session) must exist in
#      $OPENSHELL_NAMESPACE.
#
# ROUTE_HOST here is onboarding-web's OWN route (not the gateway's) — set
# it to whatever you registered as this client's exact redirectUri host in
# realm-export.json, since Keycloak rejects any mismatch.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
: "${KEYCLOAK_HOST:?set KEYCLOAK_HOST in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${ONBOARDING_WEB_ROUTE_HOST:?set ONBOARDING_WEB_ROUTE_HOST in .env — must match the openshell-onboarding-web Keycloak client redirectUri host exactly}"

if ! oc -n "$OPENSHELL_NAMESPACE" get secret onboarding-web-admin-session >/dev/null 2>&1; then
  echo "Missing Secret 'onboarding-web-admin-session' in namespace $OPENSHELL_NAMESPACE." >&2
  echo "Run scripts/10-bootstrap-onboarding-web-admin.sh first." >&2
  exit 1
fi

helm upgrade --install onboarding-web "$DEMO_DIR/onboarding-web" \
  --namespace "$OPENSHELL_NAMESPACE" \
  --set "route.host=${ONBOARDING_WEB_ROUTE_HOST}" \
  --set "keycloak.host=${KEYCLOAK_HOST}" \
  --set "keycloak.realm=${KEYCLOAK_REALM}"

oc -n "$OPENSHELL_NAMESPACE" rollout status deployment/onboarding-web

echo "onboarding-web deployed: https://${ONBOARDING_WEB_ROUTE_HOST}/"
echo "Users onboard themselves by visiting that URL and signing in —"
echo "their workspace/provider/sandbox must already have been fully"
echo "provisioned by an admin first (see README.md steps 3.0/3a/4/5)."
