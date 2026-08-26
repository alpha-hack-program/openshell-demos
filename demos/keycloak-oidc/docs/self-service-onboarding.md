# Self-service onboarding — design notes

Status: **DECIDED (for this demo's scale) — Option B, not yet implemented.**
This captures the architecture discussion and security analysis behind
replacing the operator-run `onboard` CLI with a self-service flow. At this
demo's scale (roughly up to ~50 users), Option B below is the agreed
direction. Option A remains the answer for much larger deployments and is
detailed separately in
[Self-service onboarding at scale — Option A](self-service-onboarding-option-a-at-scale.md),
since it depends on an upstream limitation that isn't resolved yet — see
[Open questions](#open-questions). No implementation has started on either
path.

## Table of contents

- [Problem statement](#problem-statement)
- [Today's flow, and why it can't just become self-service as-is](#todays-flow-and-why-it-cant-just-become-self-service-as-is)
- [The core principle: authentication is not authorization to provision](#the-core-principle-authentication-is-not-authorization-to-provision)
- [How a security-conscious company structures this](#how-a-security-conscious-company-structures-this)
- [A concrete gap in the existing code](#a-concrete-gap-in-the-existing-code)
- [Architecture options considered](#architecture-options-considered)
- [Recommendation](#recommendation)
- [Open questions](#open-questions)
- [References](#references)

## Problem statement

`util/onboard/` (see its [README](../../../util/onboard/README.md)) bridges
the gap between Keycloak (which requires a real browser-based OAuth login to
obtain an offline refresh token) and OpenShell's Providers v2 (which manages
that token's lifecycle but never obtains one itself). Today it's
operator-run: an admin launches the binary, a browser tab opens where the
*target user* (a banker, in this demo) logs in as themselves, and the
operator's own terminal — holding a Platform Admin OpenShell session — does
the `provider create` / `refresh configure` / `refresh rotate` calls.

The question explored here: can this become a web app where the user
onboards themselves end-to-end — visits a URL, logs in via Keycloak as
themselves, ends up with a working OpenShell provider — with no admin
running anything on their behalf *at onboarding time*?

## Today's flow, and why it can't just become self-service as-is

Confirmed live against this demo (`docs/headless-browser-automation.md`
lines 180-187): CLI sessions authenticated as a plain Keycloak
`openshell-user` role are denied *every* gateway operation, including
read-only `sandbox list`, with `"not a member of workspace 'default'"`. Only
the identity holding `openshell-admin` (Platform Admin) has workspace
membership. Per
[Workspace isolation](../README.md#workspace-isolation), `provider create`,
`provider refresh configure/rotate`, and `workspace create`/`member add`
all require Platform Admin, even inside a user's own workspace.

So a self-service web app's backend necessarily needs to perform these
calls on behalf of whoever just authenticated — which means the backend
itself must hold Platform Admin-equivalent OpenShell credentials,
continuously, not just for the duration of one operator-run command. That's
the entire crux of this design problem.

Two supporting facts from the current stack:

- The gateway only speaks gRPC, authenticated via a JWT in the
  `authorization` header (`demos/base/README.md` lines 95-124). There's no
  documented REST/HTTP admin API to call instead of shelling out to the
  `openshell` CLI binary — any backend service would do what `onboard` does:
  exec the CLI, or reimplement its gRPC calls.
- The `openshell` CLI persists its session as files, not in-memory:
  `$XDG_CONFIG_HOME/openshell/gateways/<name>/oidc_token.json`
  (`docs/headless-browser-automation.md` line 163). A backend can hold an
  admin identity by owning one of these files in a mounted secret, refreshed
  like any other OAuth session.
- `openshell-gateway`'s Keycloak client already has `serviceAccountsEnabled:
  true` with `standardFlowEnabled`/`directAccessGrantsEnabled` both `false`
  (`keycloak/realm-export.json`, client `openshell-gateway`) — i.e. it's
  already shaped for a client-credentials (service-account) grant, which is
  the natural way to give a backend its own non-human admin-equivalent
  credential, distinct from the demo's human `openshell-admin` user. Whether
  the gateway/CLI actually accepts a client-credentials-flow token for
  gRPC auth is unverified — flag as **[VERIFY]** before relying on it.
- There's repo precedent for an always-on backend service living beside the
  gateway as a plain Kubernetes Deployment, not a sandbox:
  `demos/keycloak-oidc/mcp-servers/templates/deployment.yaml` +
  `service.yaml`. Sandboxes are the wrong shape for this anyway — they're
  ephemeral and per-user, not meant to hold a shared standing credential.

## The core principle: authentication is not authorization to provision

Keycloak login proves *who someone is*. It says nothing about *whether they
should be handed a working OpenShell workspace* with API credentials and
MCP tool access to financial/CRM/KYC data — which this demo's own persona
(a bank) would treat as a compliance-relevant entitlement, not a login
perk. Any security-first design keeps those two decisions separate and
never lets "successfully authenticated" silently become "provisioned."

Designs that auto-provision on first login, gated only by membership in a
Keycloak group an admin maintains ad hoc, collapse this distinction: they
look like a real control (there's a check!) but the check's authority is
only as strong as whatever governs that group — and "an admin remembers to
click a checkbox" is not a control, it's a manual step wearing one's
clothes.

## How a security-conscious company structures this

1. **Provisioning follows a Joiner-Mover-Leaver (JML) process, not a login
   side-effect.** In regulated environments — and banking is this demo's
   own theme, where segregation-of-duties requirements are common — "a
   person authenticates and thereby grants themselves new system access" is
   generally not acceptable standing alone. Access is normally driven by an
   upstream system of record (HRIS, an IGA tool, a ticket), and any
   Keycloak group used for gating is *synced from* that system, not
   maintained ad hoc.

2. **The standing admin-equivalent credential is the biggest new attack
   surface, and OpenShell's RBAC model can't shrink it for you.** Per
   [Workspace isolation](../README.md#workspace-isolation) there are only
   three tiers — Platform Admin, Workspace Admin, Workspace User — nothing
   narrower like "can create workspaces and providers, cannot read other
   workspaces' sandboxes or set policy." A production version of this
   backend has to compensate at the infra layer instead:
   - Credential lives in a real secret store (Vault / cloud KMS), rotated
     automatically, never logged, never exposed via any debug/error path.
   - The backend pod gets a locked-down `NetworkPolicy` (egress only to
     Keycloak + the gateway's gRPC port), runs as non-root, holds no other
     cluster RBAC.
   - Treat the pod as a Tier-0 asset: if it's compromised, the attacker has
     Platform Admin over the whole gateway. Every other decision should
     assume that's the thing being defended.

3. **A human or policy-as-code approval sits *between* authentication and
   provisioning.** Two common enterprise patterns: (a) self-service
   *requests* access, a human or policy engine approves, automation
   fulfills it — self-service UX with a real decision point; or (b)
   provisioning stays tied to an already-governed out-of-band process (HR
   onboarding triggers IGA, IGA syncs a Keycloak group or calls a
   provisioning API directly) — no human touches a CLI, but no unmediated
   "user logs in → gets access" path either.

4. **Abuse/rate-limiting matters once this is reachable by "anyone in the
   realm," not just an admin's terminal** — provisioning triggers real
   compute (sandboxes, inference routes). Per-subject and per-IP rate
   limits, and ideally requiring MFA in Keycloak before granting a token
   that leads to spend, are standard.

5. **Audit trail moves from "an admin ran a command" (implicit, in shell
   history) to explicit structured events** shipped to a system the
   provisioning service itself can't edit (SIEM/log aggregator) — capturing
   subject, workspace, the entitlement/group that authorized it, source IP,
   timestamp.

## A concrete gap in the existing code

`util/onboard/src/main.rs`'s `build_auth_url` (line 114) has **no `state`
parameter and no PKCE** (`code_challenge`). That's acceptable for the CLI
today because the callback listener only binds `127.0.0.1` — physical/
process co-location on the same machine is the implicit CSRF mitigation. A
web app collecting OAuth callbacks from arbitrary users' browsers over the
network **must** add `state` (CSRF protection) and PKCE (authorization-code
interception protection), and register an exact `redirect_uri` in the
Keycloak client rather than relying on convention. This applies regardless
of which architecture option below is chosen.

## Architecture options considered

**A — Full self-service, admin-backend with allowlist, auto-provision on
first login.** A Deployment (chart-shaped like `mcp-servers`) exposes "Sign
in with Keycloak." On callback, checks the user's token for membership in a
Keycloak group (e.g. `openshell-eligible`, mirroring the existing
`compatibility-users` pattern); if eligible, auto-creates workspace,
membership, provider, and refresh config using its own service-account
admin credential. The group is both the authorization gate and (via
Keycloak's own admin log) the audit trail.
*Assessment:* collapses authentication into authorization unless the group
is itself governed by an upstream process (IGA/HR sync) — out of scope for
a demo cluster. Biggest blast-radius change: a persistent service holds
admin-equivalent power continuously.

**B — Token-attach only; workspace provisioning stays admin-run.** Step 3.0
(`workspace create` + `member add`) stays exactly as-is, admin-only, out of
band. The web app does only what `onboard --token-only` plus the attach
steps do: user logs in as themselves, backend takes the resulting offline
token and calls `provider create`/`refresh configure`/`refresh rotate`
against a workspace that must already exist and already have this user as
a member — 403 otherwise. "Workspace already exists for this user" is the
allowlist, identical to today's implicit gate, just automated instead of
admin-run per user.
*Assessment:* smallest trust-boundary shift; it's `onboard`'s existing
command sequence moved server-side and triggered by the user's own login.
Provisioning stays a distinct, deliberate admin act — the enterprise-correct
shape. Doesn't remove the admin bottleneck (an admin still pre-provisions
every workspace by name).

**C — Full self-service, gated via the Keycloak admin console instead of
CLI.** Functionally the same as A — same standing credential, same
collapsed authn/authz boundary — but the admin's one-time gating action is
"add user to a group in Keycloak's console" rather than a cluster command.
Cosmetically lighter-weight; substantively the same problem, since the
security posture depends entirely on what governs that group, not on how
the admin's click is performed.

## Recommendation

**Decision: Option B, hardened with PKCE + `state` and structured audit
logging, for this demo's current scale (roughly up to ~50 users).** It
already matches how enterprises structure this: provisioning is a
distinct, governed act; self-service only automates the
credential-acquisition dance for identities already decided to be
onboarded. It also sidesteps a real blocker described below.

A worthwhile stretch beyond B, if the demo later wants to show the
JML/approval pattern explicitly without collapsing authn into authz: a
self-service *request* flow (user requests access, an admin approves via a
small queue), where approval — not login — is what triggers the
still-admin-gated workspace provisioning automatically. This preserves a
real decision point while giving users a self-service front door.

Options A and C are not recommended as-is for this demo's persona (a bank)
or its current scale; they would need the eligibility group to be fed by a
governed upstream process to be defensible, which is out of scope for a
single-cluster demo. They also currently run into the service-account
limitation in [Open questions](#open-questions) below. Past the point
where admin-run pre-provisioning becomes the bottleneck (larger user
counts, higher onboarding cadence), Option A is the direction to revisit —
see
[Self-service onboarding at scale — Option A](self-service-onboarding-option-a-at-scale.md)
for what it would take to get there.

## Open questions

- **Partially resolved, still [VERIFY] against upstream.** Per the demo
  owner, OpenShell reportedly has a known limitation where a Keycloak
  service-account (client-credentials grant) token cannot carry the
  `openshell-admin` claim needed for gateway admin operations — i.e. there
  is currently no clean non-human path to a Platform Admin-equivalent
  credential. This is relayed from memory, not yet confirmed against the
  actual OpenShell issue tracker or docs — treat as **[VERIFY]** before
  relying on it operationally. It's also the reason Option A is deferred
  rather than pursued now: its standing backend credential would have to
  be a long-lived, human-admin-flavored token rather than a lightweight
  service account — see
  [Self-service onboarding at scale — Option A](self-service-onboarding-option-a-at-scale.md)
  for what that implies.
- Where exactly does the workspace-naming convention (today: named after
  `USER_ID`) get validated against the authenticated subject in option B,
  to prevent one user's login from being able to target another user's
  workspace name via a crafted request?
- Does this replace `onboard`, or does `onboard` remain the CLI/scripting
  path with the web app as the recommended interactive path — mirroring how
  `docs/manual-onboarding.md` today documents the manual equivalent of
  `onboard`?
- Who owns the pre-provisioning step in option B day-to-day — does it stay
  a raw `openshell workspace create`/`member add` pair (as today), or does
  it move into a slightly friendlier admin tool without changing who's
  allowed to trigger it?

## References

- [Self-service onboarding at scale — Option A](self-service-onboarding-option-a-at-scale.md) —
  what Option A would need in order to be defensible past this demo's
  current scale, and the known blocker standing in its way today.
- [`util/onboard/README.md`](../../../util/onboard/README.md) — the CLI tool
  this design would give a self-service front end to.
- [`docs/manual-onboarding.md`](manual-onboarding.md) — command-by-command
  equivalent of what `onboard` does; the same relationship a self-service
  app would likely have to `onboard` itself.
- [Workspace isolation](../README.md#workspace-isolation) — why Platform
  Admin is required for every provisioning call today.
- [`docs/headless-browser-automation.md`](../../../docs/headless-browser-automation.md)
  — source of the "per-user CLI sessions can't self-service anything"
  finding.
- [`docs/sandbox-service-patterns.md`](../../../docs/sandbox-service-patterns.md)
  — why an always-on backend belongs as a Deployment, not a sandbox.
