# Manual banker onboarding (password grant + step-by-step Providers v2 setup)

The main guide's [step 3](../README.md#3-onboard-a-banker) onboards each
banker with the `onboard` CLI tool — it drives a real browser-based OAuth
login as the banker themselves and wires the resulting token into OpenShell
automatically. This document covers the same ground **without** that tool:
how to obtain a refresh token via a password grant instead of a real login,
and the exact sequence of OpenShell CLI commands `onboard` runs on your
behalf. Read this if you want to understand what `onboard` does internally,
need to onboard without the binary, or are working in a fully-controlled
demo/test environment where a password-grant shortcut is acceptable.

**Prerequisite:** each banker still needs their own OpenShell workspace
before either of the steps below — see
[step 3.0](../README.md#step-30--create-the-users-own-workspace)
in the main guide. This is a Platform Admin operation and only needs to
run once per banker.

## Obtain a refresh token via password grant (demo only)

Only works because you control both sides and know the demo banker's
password. Not viable in production — the operator must never know user
credentials. This is why the main guide defaults to `onboard`'s real
browser login instead.

```bash
# Terminal A — admin
openshell whoami   # confirm: Name: openshell-admin

source .env

USER_ID="alice"
USER_PASS="<the-bankers-password>"

REFRESH_TOKEN=$(curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}/protocol/openid-connect/token" \
  -d "grant_type=password" \
  -d "client_id=${KEYCLOAK_CLIENT_ID_CLI}" \
  -d "username=${USER_ID}" \
  -d "password=${USER_PASS}" \
  -d "scope=openid offline_access" \
  | jq -r '.refresh_token')
```

Then continue below to store the token in OpenShell.

## Store the refresh token in OpenShell

This is what OpenShell owns. The **provider profile** defines the credential
refresh strategy — how the gateway obtains fresh access tokens from Keycloak
on the user's behalf. The **provider instance** stores each user's refresh
token and is linked to the profile.

The profile defines both the credential refresh strategy and the
**endpoint binding** — which endpoints the proxy is allowed to inject the
credential for. In 0.0.106+, credentials are only delivered to sandboxes
when the profile includes matching endpoints (this is a security
enhancement that prevents credentials from leaking to unintended
destinations). Envoy RBAC at each MCP server still enforces role-based
access — the endpoint binding only tells the proxy "you may inject this
credential for requests to these hosts."

**Who runs this:** Terminal A — admin, throughout (provider profile
import/create/refresh-configure/refresh-rotate all require the Platform
Admin role, even inside a banker's own workspace — see
[Workspace isolation](../README.md#workspace-isolation)).

First, import the provider profile. The profile at
`providers/user-refresh-profile.yaml` contains two placeholders:
`<keycloak-host>` in `token_url` and `<openshell-namespace>` in the MCP
server endpoint hostnames — `onboard` substitutes these for you
automatically; here you do it by hand with `sed`:

```bash
# Terminal A — admin
openshell whoami   # confirm: Name: openshell-admin

source .env

USER_ID="alice"

TMPFILE=$(mktemp --suffix=.yaml)
sed -e "s|<keycloak-host>|${KEYCLOAK_HOST}|" \
    -e "s|<openshell-namespace>|${OPENSHELL_NAMESPACE}|" \
    providers/user-refresh-profile.yaml > "$TMPFILE"
openshell provider profile import -f "$TMPFILE" --workspace "${USER_ID}"
rm -f "$TMPFILE"
```

Then create a provider for the user and configure automatic token refresh —
note `--workspace "${USER_ID}"` on every command, targeting the workspace
created in
[step 3.0](../README.md#step-30--create-the-users-own-workspace):

```bash
# Create the provider — this links the user to the profile's refresh strategy
openshell provider create \
  --name "user-${USER_ID}" \
  --type user-scoped-api \
  --credential USER_ACCESS_TOKEN=pending \
  --workspace "${USER_ID}"

# Store the user's refresh token and configure automatic rotation
openshell provider refresh configure "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN \
  --strategy oauth2-refresh-token \
  --material client_id="${KEYCLOAK_CLIENT_ID_CLI}" \
  --material refresh_token="${REFRESH_TOKEN}" \
  --secret-material-key refresh_token \
  --workspace "${USER_ID}"

# Trigger the first rotation to verify everything works
openshell provider refresh rotate "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN \
  --workspace "${USER_ID}"
```

From this point forward, the gateway's refresh worker automatically mints
short-lived access tokens and rotates the refresh token whenever the IdP
returns a new one. The user just does `openshell sandbox connect` and gets a
sandbox with a live credential — they never see a token.

> **Why three separate commands?** A provider profile can define *multiple*
> credentials, each with its own refresh strategy, token endpoint, and
> timing. `provider create` registers the credential keys the provider will
> manage. `refresh configure` binds a specific strategy and the per-user
> material (the actual refresh token) to each credential key — one call per
> key. `refresh rotate` triggers the first token exchange to verify the
> wiring. The `--strategy` flag on `refresh configure` is not redundant
> with the profile: when a profile declares several credentials with
> different strategies, the flag tells the gateway which strategy applies to
> which credential key on this particular provider instance.

> The script `scripts/03-onboard-user.sh <user-id> <refresh-token>` wraps
> these same commands — and also does
> [step 3.0](../README.md#step-30--create-the-users-own-workspace)
> for you (workspace create + membership), so it's safe to run standalone.
