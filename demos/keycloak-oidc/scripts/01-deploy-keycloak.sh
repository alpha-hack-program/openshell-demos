#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Source root .env for CLUSTER_APPS_DOMAIN
ROOT_ENV="$SCRIPT_DIR/../../../.env"
if [[ -f "$ROOT_ENV" ]]; then
  set -a; source "$ROOT_ENV"; set +a
fi
: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in the root .env}"

KEYCLOAK_REALM="${KEYCLOAK_REALM:-openshell}"
KEYCLOAK_HOST="${KEYCLOAK_HOST:-keycloak.${CLUSTER_APPS_DOMAIN}}"
KEYCLOAK_CLIENT_SECRET="${KEYCLOAK_CLIENT_SECRET:-$(openssl rand -base64 32)}"

echo "Deploy Keycloak via your chart of choice, then confirm it's reachable at:"
echo "  https://$KEYCLOAK_HOST"
echo

TMP=$(mktemp)
sed "s#__REPLACE_AT_DEPLOY_TIME__#${KEYCLOAK_CLIENT_SECRET}#" \
  "$SCRIPT_DIR/../keycloak/realm-export.template.json" > "$TMP"

echo "Realm JSON prepared at $TMP with the real client secret substituted."
echo "Import it via the Keycloak admin console or kcadm.sh:"
echo "  kcadm.sh create realms -f $TMP"
echo
echo "Then, manually (not scripted here — demo-only shortcut):"
echo "  create 2-3 demo users with 'offline_access' in scope,"
echo "  so scripts/03-onboard-user.sh can capture a refresh token for each."
echo
echo "======================================================================"
echo "Add these values to your demos/keycloak-oidc/.env file:"
echo "======================================================================"
echo ""
echo "KEYCLOAK_HOST=${KEYCLOAK_HOST}"
echo "KEYCLOAK_REALM=${KEYCLOAK_REALM}"
echo "KEYCLOAK_CLIENT_ID_CLI=openshell-cli"
echo "KEYCLOAK_CLIENT_ID_GATEWAY=openshell-gateway"
echo "KEYCLOAK_CLIENT_SECRET=${KEYCLOAK_CLIENT_SECRET}"
echo ""
echo "======================================================================"
