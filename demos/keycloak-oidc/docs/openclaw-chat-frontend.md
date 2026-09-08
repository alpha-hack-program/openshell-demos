# OpenClaw chat frontend (optional, experimental)

> **Caveats — read before deploying.** OpenClaw
> (`github.com/openclaw/openclaw`) is an independent, third-party
> open-source project, not an NVIDIA/OpenShell component. Its entire
> Kubernetes deployment story is community-maintained; OpenClaw's own
> official docs describe their raw-manifest Kubernetes guide as "not a
> production-ready deployment." OpenClaw's community threat model is
> explicitly high-risk — full shell, file, and network access driven by an
> LLM processing untrusted content — which is exactly what OpenShell
> sandboxing exists to contain. This doc demonstrates that containment, it
> isn't a production deployment recipe. It has only been validated for a
> single banker (alice); repeating it for others is described but not yet
> tested end to end (see the note at the end of this doc).

**What this is.** OpenClaw is a chat-driven AI agent with its own
"Gateway" service (its web UI + agent runtime, port 18789 — an unrelated
namesake of the OpenShell gateway the main guide revolves around, easy to
confuse, so this doc always spells out which one it means). OpenClaw does
**not** run inside an OpenShell sandbox. Its OpenShell plugin works the
other way around: the OpenClaw Gateway process shells out to the
`openshell` **CLI** and executes agent commands over **SSH** into whatever
sandbox that CLI creates or reuses. Its config
(`plugins.entries.openshell.config`: `gateway`, `gatewayEndpoint`,
`workspace`, `command`) has no auth field of its own — authentication is
entirely whatever `openshell` CLI session already exists for the OS user
running the OpenClaw Gateway process. That means no Kubernetes
ServiceAccount/Role/RoleBinding is needed for OpenClaw to reach the
gateway; what it actually needs is a working, pre-provisioned CLI session
mounted into its pod — the same problem
[`onboarding-web`](../onboarding-web/) already solves for its own standing
admin session, reused here for a single banker's session instead.

OpenClaw also has no native OIDC login for its own chat UI (an open
upstream feature request as of this writing). The documented supported SSO
path is **trusted-proxy auth**: an OIDC-aware reverse proxy authenticates
against an IdP and forwards a trusted identity header, and OpenClaw checks
that header against an allowlist. This doc runs an `oauth2-proxy` sidecar
in front of OpenClaw for exactly that, reusing the demo's Keycloak realm
via a dedicated confidential client (`openshell-openclaw-proxy`).

**What gets deployed:** one `openclaw` Helm release
(`demos/keycloak-oidc/openclaw/`) in the same namespace as the rest of
this demo, running as **alice** — reusing her existing workspace, sandbox
(`claude-alice`), and MCP config from [step
3](../README.md#3-onboard-a-banker)/[step
4](../README.md#4-deploy-mcp-servers) of the main guide. It does not touch
anything else in the demo.

## Prerequisites

- Alice fully onboarded per the main guide's steps 3.0/3a (or 3b)/4/5 —
  her workspace, provider, `claude-alice` sandbox, and MCP config must
  already work (confirm with the [Useful
  commands](../README.md#useful-commands-verify-all-bankers-are-onboarded)
  block) before starting this doc.
- A container registry you can push to (same as every custom-image recipe
  in this repo — see
  [`docs/sandbox-service-patterns.md`](../../../docs/sandbox-service-patterns.md)).
- An Anthropic (or Anthropic-compatible) API key. **Confirmed**: setting
  `ANTHROPIC_API_KEY`/`ANTHROPIC_BASE_URL` as container env vars is
  unreliable across OpenClaw gateway versions
  (`openclaw/openclaw#56679`) — the chart instead renders
  `models.providers.anthropic.{apiKey,baseUrl}` directly into
  `openclaw.json` (see `openclaw/templates/secret-openclaw-config.yaml`),
  which is documented as the reliable override path and works with the
  demo's existing BYO-LLM DeepSeek endpoint
  (`ANTHROPIC_BASE_URL=https://api.deepseek.com/anthropic` in `.env`) —
  leave `ANTHROPIC_BASE_URL` unset to use a real Anthropic key against the
  real API instead.

## 1. Build and push the custom image

OpenClaw's official image doesn't include the `openshell` CLI, so
`demos/keycloak-oidc/images/openclaw-openshell/Containerfile` layers it on
top, following this repo's [custom-image
convention](../../../docs/sandbox-service-patterns.md):

```bash
# Download a musl static Linux x86_64 openshell CLI build (see the main
# guide's "Installing the CLI" for the release asset naming convention)
# into this directory before building:
cp /path/to/openshell demos/keycloak-oidc/images/openclaw-openshell/

REGISTRY="quay.io/atarazana"   # replace with your own
IMAGE="${REGISTRY}/openclaw-openshell:2026.7.1-2-slim-openshell1"
podman build -t "$IMAGE" \
  -f demos/keycloak-oidc/images/openclaw-openshell/Containerfile \
  demos/keycloak-oidc/images/openclaw-openshell/
podman push "$IMAGE"
```

Update `demos/keycloak-oidc/openclaw/values.yaml`'s `image.repository`/
`image.tag` to match.

## 2. Bootstrap alice's OpenShell CLI session for OpenClaw

```bash
source .env
./scripts/12-bootstrap-openclaw-alice-session.sh
```

A browser opens — log in as **alice** (her existing banker account, not
admin). This packages her mTLS + OIDC session into a tarball, same
mechanism as `10-bootstrap-onboarding-web-admin.sh` but for a banker
instead of the onboarding-web service identity. Follow the script's
printed `oc create secret generic openclaw-alice-session ...` command.

## 3. Create the `openshell-openclaw-proxy` Keycloak client and remaining Secrets

**If you're setting this demo up fresh** (realm not yet imported), the
`openshell-openclaw-proxy` client is already in `keycloak/realm-export.json`
and `scripts/01-deploy-keycloak.sh` renders/generates its secret as part of
the main guide's [step 1a](../README.md#1a-render-the-realm-json) — no
extra action needed here beyond noting the secret it prints.

**If this demo's realm is already live** (the common case — you're adding
OpenClaw to an existing deployment), do **not** re-run the full realm
import: `realm-export.json` reflects only what was true at render time,
and re-importing it against a realm that's been live for hours risks
clobbering state that has since evolved (rotated provider tokens, active
sessions). Create just the one new client via the Admin REST API instead —
fully additive, verified live to leave every other client/user untouched:

```bash
source .env
KEYCLOAK_ADMIN_TOKEN=$(curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/realms/master/protocol/openid-connect/token" \
  -d "grant_type=password" -d "client_id=admin-cli" \
  -d "username=${KEYCLOAK_ADMIN_USER}" -d "password=${KEYCLOAK_ADMIN_PASSWORD}" \
  | jq -r '.access_token')

OPENCLAW_ROUTE_HOST="${OPENCLAW_ROUTE_HOST:-openclaw-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}}"
OPENCLAW_PROXY_CLIENT_SECRET=$(openssl rand -hex 32)

curl -sk -X POST "https://${KEYCLOAK_HOST}/admin/realms/${KEYCLOAK_REALM}/clients" \
  -H "Authorization: Bearer ${KEYCLOAK_ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d "{
    \"clientId\": \"openshell-openclaw-proxy\",
    \"publicClient\": false,
    \"protocol\": \"openid-connect\",
    \"secret\": \"${OPENCLAW_PROXY_CLIENT_SECRET}\",
    \"serviceAccountsEnabled\": false,
    \"standardFlowEnabled\": true,
    \"directAccessGrantsEnabled\": false,
    \"redirectUris\": [\"https://${OPENCLAW_ROUTE_HOST}/oauth2/callback\"]
  }"
```

Either way, create the oauth2-proxy Secret from that client secret plus a
random cookie secret. **Use `--from-literal`, not `--from-file` +
`echo > file`** — confirmed live that `echo` appends a trailing newline,
`--from-file` stores the file's raw bytes (newline included), oauth2-proxy
reads the env var byte-for-byte and sends `"<secret>\n"` to Keycloak, and
Keycloak's exact-string secret comparison then rejects it with
`unauthorized_client`/`"Invalid client or Invalid client credentials"` —
a failure that's easy to misdiagnose as a genuinely wrong secret, since
shell `$(...)` command substitution silently strips the trailing newline
from both sides of any comparison you might run to sanity-check it:

```bash
oc -n "$OPENSHELL_NAMESPACE" create secret generic openclaw-oauth2-proxy-secrets \
  --from-literal=client-secret="$OPENCLAW_PROXY_CLIENT_SECRET" \
  --from-literal=cookie-secret="$(openssl rand -base64 32 | head -c 32)"
```

And the LLM-key secret — `ANTHROPIC_API_KEY` here is the source of truth
that `scripts/13-deploy-openclaw.sh` reads back out at deploy time (see
[Prerequisites](#prerequisites) above for why it's passed through
`--set-string` into the config file rather than as a container env var).
**No `OPENCLAW_GATEWAY_TOKEN`** — confirmed live that a configured shared
token and `gateway.auth.mode: "trusted-proxy"` (this chart's oauth2-proxy
sidecar) are mutually exclusive; setting both blocks gateway startup with
`gateway: Invalid input`:

```bash
oc -n "$OPENSHELL_NAMESPACE" create secret generic openclaw-secrets \
  --from-literal=ANTHROPIC_API_KEY="$ANTHROPIC_API_KEY"
```

## 4. Deploy

```bash
./scripts/13-deploy-openclaw.sh
```

Checks all three Secrets exist, grants the `openclaw` ServiceAccount the
`anyuid` SCC, then `helm upgrade --install`s the chart and waits on the
rollout. The SCC grant is required because — unlike OpenShell's own
images — `ghcr.io/openclaw/openclaw` hardcodes `/home/node/.openclaw` to
`0700`, owned by `uid=1000/gid=1000` (confirmed by inspecting the image
directly), so it can't run under OpenShift's default arbitrary-UID
convention. `anyuid` is a narrower grant than the `privileged` SCC already
used for `openshell-sandbox` in the main guide's [step
2a](../README.md#2a-helm-install).

## 5. Verify

1. Visit the printed Route URL in a browser — confirm it redirects to
   Keycloak, log in as alice, confirm it lands back on OpenClaw's chat UI
   (proves the oauth2-proxy sidecar is wired correctly).
2. Send a chat prompt that requires shell execution. From Terminal B
   (alice's own terminal), independently confirm the command actually ran
   inside `claude-alice` — not a local container OpenClaw might otherwise
   default to — by checking the sandbox's own exec history/logs.
3. Re-run the main guide's existing cross-workspace denial check from
   alice's identity (`openshell sandbox exec -n claude-bob --workspace
   alice -- echo blocked` — see [Workspace
   isolation](../README.md#workspace-isolation)) to confirm OpenClaw's
   access doesn't create a new isolation bypass beyond what alice could
   already do herself.

## Repeating for another banker

Not implemented or tested in this pass — the pattern, if you want it:
rerun steps 2-4 with a second banker (e.g.
`./scripts/12-bootstrap-openclaw-alice-session.sh bob`, which the script
supports via a positional user-ID argument), install a second Helm release
(`helm upgrade --install openclaw-bob ...`) with its own `route.host`,
`openshell.workspace=bob`, and `aliceSessionSecretName=openclaw-bob-session`,
and register a second redirect URI (or a second Keycloak client) for that
Route. Remember OpenClaw is single-instance upstream — this means N
separate Helm releases, not a shared multi-tenant OpenClaw deployment.
