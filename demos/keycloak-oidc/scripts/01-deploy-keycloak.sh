#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

# Source root .env for CLUSTER_APPS_DOMAIN
ROOT_ENV="$SCRIPT_DIR/../../../.env"
if [[ -f "$ROOT_ENV" ]]; then
  set -a; source "$ROOT_ENV"; set +a
fi

# Source demo .env so previously saved values are reused on re-run
DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in the root .env}"
: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"

KEYCLOAK_REALM="${KEYCLOAK_REALM:-openshell}"
KEYCLOAK_HOST="${KEYCLOAK_HOST:-keycloak.${CLUSTER_APPS_DOMAIN}}"
# Same formula scripts/10-.../11-... derive independently for the actual
# deploy — keep in sync if this ever changes.
ONBOARDING_WEB_ROUTE_HOST="${ONBOARDING_WEB_ROUTE_HOST:-onboarding-web-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}}"

REALM_JSON="$DEMO_DIR/keycloak/realm-export.json"
RENDERED_JSON="$DEMO_DIR/keycloak/realm-export.rendered.json"

# realm-export.json ships with one literal placeholder:
# <onboarding-web-base-url>, in the openshell-onboarding-web client's
# redirectUris. Unlike onboard's provider-profile placeholders (substituted
# by the onboard binary at onboarding time), nothing substitutes this one
# automatically — it has to happen before the realm is imported in step 1c,
# since Keycloak enforces an exact redirect URI match and this client's
# config is otherwise static from here on. Render a real, gitignored copy
# rather than editing the checked-in template in place.
sed "s#<onboarding-web-base-url>#https://${ONBOARDING_WEB_ROUTE_HOST}#g" \
  "$REALM_JSON" > "$RENDERED_JSON"

echo "Deploy Keycloak via your chart of choice, then confirm it's reachable at:"
echo "  https://$KEYCLOAK_HOST"
echo
echo "Rendered realm JSON ready to import at:"
echo "  $RENDERED_JSON"
echo "(<onboarding-web-base-url> substituted with https://${ONBOARDING_WEB_ROUTE_HOST} —"
echo "override by exporting ONBOARDING_WEB_ROUTE_HOST before re-running this script"
echo "if you need a different onboarding-web hostname than the default convention.)"
echo
echo "The gateway client secret is hardcoded in the realm JSON"
echo "(openshell-gateway-demo-secret) — demo only."
echo
echo "Import the rendered file via the Keycloak admin console or the Admin REST API."
echo
echo "The realm includes demo users (alice, bob, charlie) — Meridian"
echo "Private Bank's bankers — with the openshell-user + banker roles"
echo "and offline_access scope. Passwords match usernames"
echo "(alice/alice, bob/bob, charlie/charlie). Alice additionally"
echo "belongs to the compatibility-users group (compatibility-user role)."
echo
echo "======================================================================"
echo "Ensure these values are in your demos/keycloak-oidc/.env file:"
echo "======================================================================"
echo ""
echo "KEYCLOAK_HOST=${KEYCLOAK_HOST}"
echo "KEYCLOAK_REALM=${KEYCLOAK_REALM}"
echo "KEYCLOAK_CLIENT_ID_CLI=openshell-cli"
echo "KEYCLOAK_CLIENT_ID_GATEWAY=openshell-gateway"
echo "KEYCLOAK_CLIENT_SECRET=openshell-gateway-demo-secret"
echo ""
echo "======================================================================"
