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

KEYCLOAK_REALM="${KEYCLOAK_REALM:-openshell}"
KEYCLOAK_HOST="${KEYCLOAK_HOST:-keycloak.${CLUSTER_APPS_DOMAIN}}"

REALM_JSON="$DEMO_DIR/keycloak/realm-export.json"

echo "Deploy Keycloak via your chart of choice, then confirm it's reachable at:"
echo "  https://$KEYCLOAK_HOST"
echo
echo "Realm JSON ready to import at:"
echo "  $REALM_JSON"
echo
echo "The gateway client secret is hardcoded in the realm JSON"
echo "(openshell-gateway-demo-secret) — demo only."
echo
echo "Import it via the Keycloak admin console or the Admin REST API."
echo
echo "The realm includes demo users (user1, user2) with the"
echo "openshell-user role and offline_access scope. Passwords"
echo "match usernames (user1/user1, user2/user2)."
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
