#!/usr/bin/env bash
set -euo pipefail
# Registers one user with their own provider instance, storing their own
# refresh token so the gateway can mint short-lived, user-scoped access
# tokens on their behalf.
#
# Usage: ./03-onboard-user.sh <user-id> <user-refresh-token>
#
# The refresh token must have been issued to the public CLI client
# (openshell-cli) with offline_access scope. Keycloak binds refresh tokens
# to the client that obtained them, so the refresh material here must use
# the same client_id — NOT the confidential gateway client.

USER_ID="${1:?usage: $0 <user-id> <user-refresh-token>}"
USER_REFRESH_TOKEN="${2:?usage: $0 <user-id> <user-refresh-token>}"
: "${KEYCLOAK_CLIENT_ID_CLI:?set in .env}"

HERE="$(dirname "$0")"
openshell provider profile import -f "$HERE/../providers/user-refresh-profile.yaml" || true

openshell provider create \
  --name "user-${USER_ID}" \
  --type user-scoped-api \
  --credential USER_ACCESS_TOKEN=pending

openshell provider refresh configure "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN \
  --strategy oauth2-refresh-token \
  --material client_id="${KEYCLOAK_CLIENT_ID_CLI}" \
  --material refresh_token="${USER_REFRESH_TOKEN}" \
  --secret-material-key refresh_token

openshell provider refresh rotate "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN

echo "Provider user-${USER_ID} created and refreshed."
echo "Attach it to a sandbox with: --provider user-${USER_ID}"
