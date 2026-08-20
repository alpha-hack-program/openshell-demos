#!/usr/bin/env bash
set -euo pipefail
# Creates the user's own workspace (if needed), grants them 'user' membership
# in it, then registers a provider instance in that workspace, storing their
# own refresh token so the gateway can mint short-lived, user-scoped access
# tokens on their behalf.
#
# Usage: ./03-onboard-user.sh <user-id> <user-refresh-token>
#
# Each user MUST get their own workspace. Workspace membership is not scoped
# to "your own sandboxes" — a 'user'-role member of a workspace can see and
# act on every sandbox in that workspace. Putting multiple users in one
# shared workspace (e.g. 'default') lets any one of them exec into any
# other's sandbox and ride their real credentials — verified live. See
# https://docs.nvidia.com/openshell/sandboxes/manage-workspaces.
#
# The refresh token must have been issued to the public CLI client
# (openshell-cli) with offline_access scope. Keycloak binds refresh tokens
# to the client that obtained them, so the refresh material here must use
# the same client_id — NOT the confidential gateway client.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

USER_ID="${1:?usage: $0 <user-id> <user-refresh-token>}"
USER_REFRESH_TOKEN="${2:?usage: $0 <user-id> <user-refresh-token>}"
: "${KEYCLOAK_CLIENT_ID_CLI:?set in .env}"
: "${KEYCLOAK_HOST:?set in .env}"
: "${KEYCLOAK_REALM:=openshell}"
: "${OPENSHELL_NAMESPACE:?set in .env}"

HERE="$SCRIPT_DIR"
WORKSPACE="${USER_ID}"

# ---------------------------------------------------------------------------
# Create the user's own workspace and grant them 'user' membership, keyed by
# their Keycloak subject (OIDC 'sub' claim = Keycloak user ID). Idempotent —
# safe to re-run.
# ---------------------------------------------------------------------------
if [[ -z "${KEYCLOAK_ADMIN_TOKEN:-}" ]]; then
  : "${KEYCLOAK_ADMIN_USER:?set KEYCLOAK_ADMIN_USER in .env or export KEYCLOAK_ADMIN_TOKEN}"
  : "${KEYCLOAK_ADMIN_PASSWORD:?set KEYCLOAK_ADMIN_PASSWORD in .env or export KEYCLOAK_ADMIN_TOKEN}"
  KEYCLOAK_ADMIN_TOKEN=$(curl -sk -X POST \
    "https://${KEYCLOAK_HOST}/realms/master/protocol/openid-connect/token" \
    -d "grant_type=password" \
    -d "client_id=admin-cli" \
    -d "username=${KEYCLOAK_ADMIN_USER}" \
    -d "password=${KEYCLOAK_ADMIN_PASSWORD}" \
    | jq -r '.access_token')
fi

KC_USER_ID=$(curl -sk -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users?username=${USER_ID}&exact=true" \
  | jq -r '.[0].id // empty')
[ -n "$KC_USER_ID" ] || { echo "No Keycloak user found for '${USER_ID}' in realm ${KEYCLOAK_REALM}"; exit 1; }

openshell workspace create --name "${WORKSPACE}" 2>/dev/null || true
openshell workspace member add --workspace "${WORKSPACE}" --subject "${KC_USER_ID}" --role user 2>/dev/null || true

TMPFILE=$(mktemp --suffix=.yaml)
sed -e "s|<keycloak-host>|${KEYCLOAK_HOST}|" \
    -e "s|<openshell-namespace>|${OPENSHELL_NAMESPACE}|" \
    "$HERE/../providers/user-refresh-profile.yaml" > "$TMPFILE"
openshell provider profile import -f "$TMPFILE" --workspace "${WORKSPACE}" || true
rm -f "$TMPFILE"

openshell provider create \
  --name "user-${USER_ID}" \
  --type user-scoped-api \
  --credential USER_ACCESS_TOKEN=pending \
  --workspace "${WORKSPACE}" || true

openshell provider refresh configure "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN \
  --strategy oauth2-refresh-token \
  --material client_id="${KEYCLOAK_CLIENT_ID_CLI}" \
  --material refresh_token="${USER_REFRESH_TOKEN}" \
  --secret-material-key refresh_token \
  --workspace "${WORKSPACE}"

openshell provider refresh rotate "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN \
  --workspace "${WORKSPACE}"

echo "Provider user-${USER_ID} created and refreshed in workspace ${WORKSPACE}."
echo "Attach it to a sandbox with: --provider user-${USER_ID} --workspace ${WORKSPACE}"
