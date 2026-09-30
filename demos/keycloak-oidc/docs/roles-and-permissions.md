# Reproducing this demo's role model against a different OIDC provider

This demo's Envoy sidecars and the MCP Gateway's `AuthPolicy` both read
authorization data out of the caller's JWT, not out of anything
Keycloak-specific. Anyone replicating this role model against a different
OIDC server needs their IdP to produce tokens shaped the way Keycloak's
`resource_access`/`realm_access` claims are, not to run Keycloak itself.
This doc describes that contract. For this demo's own concrete role
assignments, read
[`../keycloak/realm-export.json`](../keycloak/realm-export.json) directly —
it's the reference implementation, not reproduced here.

## Two independent permission layers, two claim shapes

Every MCP server pod in this demo carries an Envoy sidecar that checks one
claim, and (when routed through the MCP Gateway) Authorino checks a
different one. Both must be present on the same token for a request to
succeed end to end:

| Layer | Enforced by | Claim | Shape |
|---|---|---|---|
| Server-level RBAC | Each server's own Envoy sidecar (`rbac` filter) | `realm_access.roles` | Flat list of realm-role names, e.g. `["banker", "mcp-portfolio-user"]` |
| Per-tool/prompt authz | MCP Gateway's `mcp-gateway-authz` AuthPolicy (Authorino, CEL) | `resource_access.<registrationNamespace>/<serverName>.roles` | Per-"client" map of role lists, e.g. `resource_access["mcp-gateway-system/mcp-portfolio"].roles = ["tool:list_my_clients", ...]` |

The Envoy layer only ever needs to know "does this caller hold role X at
all" — a realm-wide flag. The gateway layer needs to know "does this caller
hold permission X **for this specific server**" — scoped per server, using
the `<registrationNamespace>/<serverName>` string (e.g.
`mcp-gateway-system/mcp-portfolio`) as the map key, exactly matching the
`MCPServerRegistration` name from
[`../mcp-servers/README.md`](../mcp-servers/README.md#what-gets-created).

## The `tool:<name>` / `prompt:<name>` convention

Inside a `resource_access.<server>` entry, each role name is prefixed:

- `tool:<toolName>` — permission to call that tool.
- `prompt:<promptName>` — permission to fetch that prompt.

This is what `authpolicy.yaml`'s `tool-access-check`/`prompt-access-check`
CEL rules test for — see
[`token-exchange-design.md`](token-exchange-design.md) for the
full authz-path walkthrough these checks sit in. A non-Keycloak IdP must
emit role names in exactly this `tool:`/`prompt:` prefixed form under the
matching `resource_access.<server>` key for the same `AuthPolicy` to work
completely unmodified — the CEL doesn't know or care which IdP issued the
token, only its claim shape.

## Composite roles and the banker / compatibility-user split

This demo's realm role `banker` is a single role that composites over
*both* claim shapes at once: it expands into flat `realm_access.roles`
entries (`mcp-portfolio-user`, `mcp-crm-calendar-user`, etc., for the Envoy
layer) **and** into `resource_access` client-role entries (`tool:*` entries
under each `mcp-gateway-system/mcp-*` key, for the gateway layer) — see the
`banker` role's `composites` block in `realm-export.json`. Holding `banker`
alone is enough to pass both layers for the four shared servers.

`compatibility-user` is the narrower counterpart: a single realm role,
granted to one user only (Alice) via Keycloak group membership rather than
composited into `banker`, that expands into exactly one `resource_access`
entry (`mcp-gateway-system/mcp-compatibility: ["tool:calc_tax"]`) and one
Envoy-facing realm role. It's the concrete example of "an extra permission
one user has that others don't" — not shared, not inherited, not composited
into the baseline role every other banker holds.

A non-Keycloak IdP doesn't need composite roles as a mechanism — it only
needs to be able to produce, for a given user, both claim shapes
consistently. How it internally models "which roles compose into which"
(a database join, a static config, an actual composite-role feature) is
its own concern.

## What a non-Keycloak OIDC provider must produce

For this demo's `AuthPolicy`/Envoy config to work unmodified against a
different IdP, every access token it issues to a banker must carry:

1. `iss` matching whatever `keycloak.issuer` (main README's `.env`
   `KEYCLOAK_HOST`/`KEYCLOAK_REALM`) is reconfigured to point at, with a
   reachable JWKS endpoint for both the Envoy sidecars and Authorino to
   verify signatures against.
2. `realm_access.roles` — a flat array of role names — including whatever
   role each server's Envoy sidecar's `rbac` filter is configured to
   require.
3. `resource_access.<registrationNamespace>/<serverName>.roles` — one
   entry per MCP server the user may call anything on, each holding that
   user's `tool:<name>`/`prompt:<name>` grants for that specific server.
4. If token exchange is enabled (see
   [`token-exchange-design.md`](token-exchange-design.md)): an
   RFC 8693-compatible token endpoint, and an audience mapper equivalent so
   the subject token's own `aud` includes whichever client audience is
   requested during exchange.

Nothing about the `AuthPolicy` CEL, the Envoy `rbac` filter, or the
gateway's `MCPServerRegistration` naming is Keycloak-specific — they only
ever read the claim shapes above.
