#!/usr/bin/env bash
set -euo pipefail
# Path A. Registers one "customer" with their own provider instance, storing
# their own refresh token so the gateway can mint short-lived, customer-scoped
# access tokens on their behalf.
#
# Usage: ./03-onboard-customer.sh <customer-id> <customer-refresh-token>
#
# How you obtain <customer-refresh-token> is outside OpenShell: a standard
# authorization-code login for that customer against the openshell-gateway
# Keycloak client with offline_access in scope. For this demo, script it with
# curl against Keycloak's token endpoint using a demo user's password grant,
# or a scripted browser login — keep that logic here, not in the shared repo
# conventions.

CUSTOMER_ID="${1:?usage: $0 <customer-id> <customer-refresh-token>}"
CUSTOMER_REFRESH_TOKEN="${2:?usage: $0 <customer-id> <customer-refresh-token>}"
: "${KEYCLOAK_CLIENT_ID_GATEWAY:?set in .env}"
: "${KEYCLOAK_CLIENT_SECRET:?set in .env}"

HERE="$(dirname "$0")"
openshell provider profile import -f "$HERE/../providers/customer-refresh-profile.yaml" || true

openshell provider create \
  --name "customer-${CUSTOMER_ID}" \
  --type customer-scoped-api \
  --credential CUSTOMER_ACCESS_TOKEN=pending

openshell provider refresh configure "customer-${CUSTOMER_ID}" \
  --credential-key CUSTOMER_ACCESS_TOKEN \
  --strategy oauth2-refresh-token \
  --material client_id="${KEYCLOAK_CLIENT_ID_GATEWAY}" \
  --material refresh_token="${CUSTOMER_REFRESH_TOKEN}" \
  --material client_secret="${KEYCLOAK_CLIENT_SECRET}" \
  --secret-material-key refresh_token \
  --secret-material-key client_secret

openshell provider refresh rotate "customer-${CUSTOMER_ID}" \
  --credential-key CUSTOMER_ACCESS_TOKEN

echo "Provider customer-${CUSTOMER_ID} created and refreshed."
echo "Attach it to a sandbox with: --provider customer-${CUSTOMER_ID}"
