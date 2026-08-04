#!/usr/bin/env bash
set -euo pipefail
: "${KEYCLOAK_HOST:?set in demos/spire-spiffe-keycloak/.env}"
: "${KEYCLOAK_REALM:?set in .env}"

# [VERIFY] Pick and pin a Keycloak Helm chart/version; deploy it in its own
# namespace. This script assumes Keycloak is already reachable at
# https://$KEYCLOAK_HOST and only handles the realm import.
echo "Deploy Keycloak via your chart of choice, then confirm it's reachable at:"
echo "  https://$KEYCLOAK_HOST"
echo

TMP=$(mktemp)
sed "s#__REPLACE_AT_DEPLOY_TIME__#${KEYCLOAK_CLIENT_SECRET:?set in .env}#" \
  "$(dirname "$0")/../keycloak/realm-export.template.json" > "$TMP"

echo "Realm JSON prepared at $TMP with the real client secret substituted."
echo "Import it via the Keycloak admin console or kcadm.sh:"
echo "  kcadm.sh create realms -f $TMP"
echo
echo "Then, manually (not scripted here — demo-only shortcut):"
echo "  create 2-3 demo users representing 'customers', with 'offline_access' in scope,"
echo "  so scripts/03-onboard-customer.sh can capture a refresh token for each."
