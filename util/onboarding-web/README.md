# onboarding-web — self-service OpenShell onboarding

## Why this exists

[`util/onboard/`](../onboard/README.md) is operator-run: an admin runs the
binary, a browser tab opens where the *target user* logs in, and the
operator's own Platform-Admin CLI session does the `provider create` /
`refresh configure` / `refresh rotate` calls. `onboarding-web` replaces
that with a small persistent web app the user visits themselves — no admin
runs anything on their behalf at onboarding time.

See
[`demos/keycloak-oidc/docs/self-service-onboarding.md`](../../demos/keycloak-oidc/docs/self-service-onboarding.md)
for the full design discussion and the decision behind this service's
narrow scope (**Option B**): admin pre-provisions everything — workspace,
provider (with placeholder credential material), sandbox, MCP config,
policies, agent harness — out of band, exactly as
[`demos/keycloak-oidc/README.md`](../../demos/keycloak-oidc/README.md)'s
steps 3.0/3a/4/5 already describe. This service's entire job is the last
mile: the user logs in via Keycloak as themselves, picks which
already-provisioned provider(s) to activate, and this service runs
`provider refresh configure` + `provider refresh rotate` against them.

**It never runs** `workspace create`, `provider create`, `provider profile
import`, or `sandbox create`. If a user's workspace/provider doesn't exist
yet, they get a clear "contact an admin" page instead — that failure *is*
the allowlist, not a bug to route around.

Because a provider is a workspace-scoped credential resource (not a
per-sandbox one), one activation per provider — typically once per user —
covers every sandbox that references it, past and future. Users don't
re-onboard per sandbox.

## Why this is a Tier-0 service

`provider refresh configure`/`refresh rotate` are Platform-Admin gated
operations even inside the target user's own workspace (confirmed live —
see [Workspace isolation](../../demos/keycloak-oidc/README.md#workspace-isolation)).
This service therefore holds a standing Platform-Admin-equivalent
`openshell` CLI session continuously, to act on behalf of whoever just
authenticated. Treat its credential, its pod, and its logs accordingly:
locked-down `NetworkPolicy`, no other cluster RBAC, secret material never
logged.

## How it differs from `onboard`

| | `onboard` | `onboarding-web` |
|---|---|---|
| Who runs it | An admin, once per user | The user, via a browser |
| Where it runs | Operator's laptop/terminal | A standing Deployment |
| OAuth callback | Loopback (`127.0.0.1:<port>`), single flow at a time | A real HTTPS redirect URI, concurrent sessions with `state` + PKCE |
| Provisions workspace/provider? | Yes, via a chained `openshell` sequence | No — assumes admin already did this |
| Own Keycloak client | `openshell-cli` (public, loopback redirects) | `openshell-onboarding-web` (public, PKCE required, exact HTTPS redirect) |

**Why public, not confidential — confirmed live, not just a design
choice:** Providers v2's refresh grant runs inside the gateway using only
the `client_id` material configured via `provider refresh configure` (no
secret). A confidential `openshell-onboarding-web` client made that grant
fail with `401 Unauthorized` from Keycloak's token endpoint on a real
cluster — exactly what `util/onboard/PROMPT.md` already warns about for
the gateway-client-vs-CLI-client mixup, just for a different client this
time. PKCE covers the authorization-code exchange's security instead,
which is the standard OAuth 2.0 pattern for public clients anyway.

## Configuration

All flags accept an equivalent env var (see `--help`). At minimum:

```bash
export KEYCLOAK_HOST=keycloak.apps.mycluster.example.com
export ONBOARDING_WEB_BASE_URL=https://onboarding-web-<namespace>.<apps-domain>
export OPENSHELL_ADMIN_XDG_CONFIG_HOME=/path/to/mounted/admin-session/config
export OPENSHELL_ADMIN_XDG_STATE_HOME=/path/to/mounted/admin-session/state
onboarding-web
```

Locally, omit the two `OPENSHELL_ADMIN_XDG_*` vars to have `openshell`
invocations inherit whatever session is already active in your shell
(useful for testing against a real cluster where you're already logged in
as an admin).

## Development

```bash
make help       # list all targets
make build      # debug build
make check      # fmt-check + clippy
make run ARGS="--help"
```

No automated test suite yet (the PKCE/OAuth logic is small enough to have
been exercised manually against a live Keycloak instance; see the parent
design doc's verification section for the end-to-end checklist).

## Deployment

See [`demos/keycloak-oidc/onboarding-web/`](../../demos/keycloak-oidc/onboarding-web/)
for the Helm chart, and [`Containerfile`](Containerfile) for the image.
The image needs both this binary and the `openshell` CLI — the gateway
only speaks gRPC, so there is no way to reach it except by shelling out to
`openshell` (same as `onboard` does).

## Releasing

Same `cargo-release` flow as `onboard`:

```bash
cargo install cargo-release   # if not already installed
make release-patch   # or release-minor / release-major
```
