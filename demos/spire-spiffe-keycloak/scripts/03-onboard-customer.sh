#!/usr/bin/env bash
set -euo pipefail
# Path A. Registers one "customer" with their own provider instance, storing
# their own refresh token so the gateway can mint short-lived, customer-scoped
# access tokens on their behalf.
#
# Usage: ./03-onboard-customer.sh <customer-id> <customer-refresh-token>
#
# The refresh token must have been issued to the public CLI client
# (openshell-cli) with offline_access scope. Keycloak binds refresh tokens
# to the client that obtained them, so the refresh material here must use
# the same client_id — NOT the confidential gateway client.

CUSTOMER_ID="${1:?usage: $0 <customer-id> <customer-refresh-token>}"
CUSTOMER_REFRESH_TOKEN="${2:?usage: $0 <customer-id> <customer-refresh-token>}"
: "${KEYCLOAK_CLIENT_ID_CLI:?set in .env}"

HERE="$(dirname "$0")"
openshell provider profile import -f "$HERE/../providers/customer-refresh-profile.yaml" || true

openshell provider create \
  --name "customer-${CUSTOMER_ID}" \
  --type customer-scoped-api \
  --credential CUSTOMER_ACCESS_TOKEN=pending

openshell provider refresh configure "customer-${CUSTOMER_ID}" \
  --credential-key CUSTOMER_ACCESS_TOKEN \
  --strategy oauth2-refresh-token \
  --material client_id="${KEYCLOAK_CLIENT_ID_CLI}" \
  --material refresh_token="${CUSTOMER_REFRESH_TOKEN}" \
  --secret-material-key refresh_token

openshell provider refresh rotate "customer-${CUSTOMER_ID}" \
  --credential-key CUSTOMER_ACCESS_TOKEN

echo "Provider customer-${CUSTOMER_ID} created and refreshed."
echo "Attach it to a sandbox with: --provider customer-${CUSTOMER_ID}"
