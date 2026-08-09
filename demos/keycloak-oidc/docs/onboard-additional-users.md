# Onboarding additional users

The demo realm JSON ships with two pre-configured users (`user1`, `user2`)
whose Keycloak roles and OpenShell providers are set up by the main guide.
This document explains how to onboard a new user beyond those two.

There are three things to do: create the user in Keycloak, obtain their
refresh token, and register it with OpenShell.

## 1. Create the user in Keycloak

Use the Admin REST API or the admin console. The user needs:
- A username and password
- The `openshell-user` realm role (baseline sandbox access)
- The `offline_access` realm role (so the refresh token doesn't expire)
- One or more MCP server roles (`mcp-server-a-user`, `mcp-server-b-user`)
  depending on which servers they should access
- Profile fields filled in (firstName, lastName, email) to avoid
  first-login prompts

```bash
source .env

ADMIN_TOKEN=$(curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/realms/master/protocol/openid-connect/token" \
  -d "grant_type=password" \
  -d "client_id=admin-cli" \
  -d "username=${KEYCLOAK_ADMIN_USER}" \
  -d "password=${KEYCLOAK_ADMIN_PASSWORD}" \
  | jq -r '.access_token')

NEW_USER="user3"
NEW_PASS="user3"

# Create the user
curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users" \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d '{
    "username": "'"${NEW_USER}"'",
    "enabled": true,
    "firstName": "'"${NEW_USER}"'",
    "lastName": "Demo",
    "email": "'"${NEW_USER}"'@demo.local",
    "emailVerified": true,
    "credentials": [{"type": "password", "value": "'"${NEW_PASS}"'", "temporary": false}],
    "requiredActions": []
  }'

# Get the user's UUID
USER_UUID=$(curl -sk \
  "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users?username=${NEW_USER}&exact=true" \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  | jq -r '.[0].id')

# Assign realm roles
for ROLE_NAME in openshell-user offline_access mcp-server-a-user; do
  ROLE_JSON=$(curl -sk \
    "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/roles/${ROLE_NAME}" \
    -H "Authorization: Bearer ${ADMIN_TOKEN}")
  curl -sk -X POST \
    "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/users/${USER_UUID}/role-mappings/realm" \
    -H "Authorization: Bearer ${ADMIN_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "[${ROLE_JSON}]"
done
```

## 2. Obtain the user's refresh token and register with OpenShell

### Option A: `onboard` tool (recommended)

The `onboard` tool handles the full flow: opens the browser for the user to
log in, obtains the refresh token, imports the provider profile, creates
the provider, configures refresh, and triggers the first rotation.

```bash
../../util/onboard/onboard.sh \
  -u "${NEW_USER}" --verbose \
  --profile providers/user-refresh-profile.yaml
```

If you're onboarding multiple users, log out of Keycloak between them (use
the logout link on the success page or an incognito window) so the browser
doesn't reuse the previous session.

### Option B: manual commands

Get a refresh token via password grant (only works when you know the
user's password):

```bash
REFRESH_TOKEN=$(curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}/protocol/openid-connect/token" \
  -d "grant_type=password" \
  -d "client_id=${KEYCLOAK_CLIENT_ID_CLI}" \
  -d "username=${NEW_USER}" \
  -d "password=${NEW_PASS}" \
  -d "scope=openid offline_access" \
  | jq -r '.refresh_token')
```

Then register with OpenShell:

```bash
# Import the provider profile (idempotent — skips if already imported)
sed "s|<keycloak-host>|${KEYCLOAK_HOST}|" providers/user-refresh-profile.yaml \
  | openshell provider profile import -f -

# Create the provider
openshell provider create \
  --name "user-${NEW_USER}" \
  --type user-scoped-api \
  --credential USER_ACCESS_TOKEN=pending

# Configure refresh — binds the user's refresh token to the provider
openshell provider refresh configure "user-${NEW_USER}" \
  --credential-key USER_ACCESS_TOKEN \
  --strategy oauth2-refresh-token \
  --material client_id="${KEYCLOAK_CLIENT_ID_CLI}" \
  --material refresh_token="${REFRESH_TOKEN}" \
  --secret-material-key refresh_token

# Trigger the first rotation to verify everything works
openshell provider refresh rotate "user-${NEW_USER}" \
  --credential-key USER_ACCESS_TOKEN
```

## 3. Create a sandbox and authorize

```bash
SERVER_NAME="mcp-server-a"
MCP_URL="http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox create --name "demo-${NEW_USER}" -- true

openshell sandbox provider attach "demo-${NEW_USER}" "user-${NEW_USER}"

openshell policy update "demo-${NEW_USER}" \
  --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
  --binary /usr/bin/curl --wait
```

Alternatively, use `scripts/07-authorize-mcp-user.sh` which also verifies
the Keycloak role before updating the policy:

```bash
./scripts/07-authorize-mcp-user.sh "${NEW_USER}" "${SERVER_NAME}"
```

## 4. Verify

```bash
openshell sandbox exec -n "demo-${NEW_USER}" --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS \
    -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'
```

Expected: `200` with MCP server capabilities.
