# Self-service onboarding at scale — Option A design notes

Status: **DRAFT, forward-looking. Not the current plan.** The current
decision for this demo is Option B — see
[Self-service onboarding — design notes](self-service-onboarding.md). This
document sketches what Option A (full self-service, backend holds a
standing admin-equivalent credential, auto-provisions on first login) would
need in order to be defensible at larger scale, and the one concrete
blocker known today. Revisit if/when the admin-run pre-provisioning step in
Option B actually becomes the bottleneck.

## When this would apply

Option B still has an admin manually run `workspace create` + `member add`
once per user before that user can self-service anything. That's fine at
this demo's scale (roughly up to ~50 users) but doesn't scale past the
point where that manual step itself becomes the bottleneck — larger user
counts, continuous onboarding, or onboarding driven by an external system
rather than a person clicking through a runbook. Past that point, Option
A's auto-provisioning is worth the added trust-boundary risk — but only if
the eligibility gate that replaces "an admin decided to onboard you" is
itself backed by something governed (see
[the JML section of the main doc](self-service-onboarding.md#how-a-security-conscious-company-structures-this)).
Auto-provisioning gated by an ungoverned Keycloak group is not an
improvement over Option B; it's a regression dressed as automation.

## Known blocker: no admin-scoped service-account token (reported, unverified)

Per the demo owner, OpenShell has a known limitation where a Keycloak
service-account (client-credentials grant) token cannot be granted the
`openshell-admin`-equivalent claim needed for gateway admin operations.
That means the confidential-client-plus-`serviceAccountsEnabled` pattern
already used by `openshell-gateway` in this demo's realm
(`keycloak/realm-export.json`) — which looks like the obvious way to hand a
backend its own non-human admin credential — does **not** actually work
for this purpose today.

**This is relayed from memory, not confirmed against the real OpenShell
issue tracker or docs — [VERIFY] before relying on it operationally.**

If true, the practical consequence is that Option A's standing backend
credential cannot be a clean, purpose-built service account. It has to be
some human-flavored session instead:

- **(a) A real admin user's credential, refreshed by automation instead of
  a person.** Functionally identical to today's `openshell-admin` demo
  user, just kept alive indefinitely instead of used interactively. The
  backend impersonates an identity that isn't really "its own."
- **(b) An offline refresh token obtained once via a real interactive admin
  login** (the same browser-based flow `onboard` already drives for
  bankers), then kept alive forever by the gateway's own Providers v2
  refresh worker — the same mechanism this demo already uses to keep
  banker tokens alive. This is the more consistent-with-the-existing-stack
  answer, since Providers v2 already knows how to do exactly this; the
  only difference is *whose* token it's refreshing.

Either way, this changes the risk profile from "the backend holds a
narrowly-scoped service-account credential" to "the backend holds a
long-lived, human-admin-flavored token" — a materially bigger commitment,
and the reason this design stays deferred rather than pursued now. Every
mitigation in the [main doc's security section](self-service-onboarding.md#how-a-security-conscious-company-structures-this)
(secret store, network policy, Tier-0 treatment) still applies, but starts
from a worse baseline than a purpose-built service account would.

## What Option A needs beyond what's in the main design doc

1. **Resolve the service-account-token limitation upstream first.** This is
   the actual precondition for a clean implementation, not a nice-to-have —
   see the `[VERIFY]` above.
2. **A governed eligibility source, not an ad-hoc Keycloak group.** Synced
   from an HRIS/IGA system or equivalent, per the JML principle in the main
   doc — an admin manually curating a Keycloak group is the same
   authz-collapse problem the main doc already flags, just moved to
   whatever scale this option targets.
3. **A deterministic, collision-safe workspace-naming policy.** Option B's
   workspace-name == user-ID convention was previously admin-chosen per
   user by hand. Auto-creation needs a mapping from the authenticated
   subject (the OIDC `sub` claim, not a mutable username) to a workspace
   name, plus a check that rejects provisioning if a workspace of that name
   already exists and isn't already this same subject's.
4. **Rate limiting and per-subject/per-IP throttling** on the provisioning
   endpoint — it now creates real compute (a workspace, and eventually
   sandboxes) reachable by anyone who can complete a Keycloak login, not
   just an admin who chose to run a command.
5. **A structured, tamper-evident audit trail** (see the main doc's
   auditing section) becomes load-bearing at this scale, since there's no
   remaining per-user admin action for anyone to eyeball or catch mistakes
   in.
6. **An automated revocation/offboarding path.** Option B needs this too,
   but it's more urgent at scale: workspace deletion and provider teardown
   when someone leaves the eligible group, ideally driven by the same
   upstream system of record that grants access, not a separate manual
   step.

## Rough shape

Same Deployment-based backend described in the main doc (a `Deployment` +
`Service` alongside the gateway, shaped like `mcp-servers/`, not a sandbox
workload), but its stored OpenShell session/refresh token follows the
human-admin-flavored pattern above rather than a lightweight service
account. The request handler performs the full sequence — `workspace
create`, `member add`, `provider create`, `refresh configure`, `refresh
rotate` — in one flow, gated by the eligibility check (item 2 above) before
any of it runs.

## Open questions

- **[VERIFY]** the service-account admin-token blocker itself, against the
  real OpenShell issue tracker/docs — confirm it's still true for the
  chart version this repo targets, since this space moves fast (see
  AGENTS.md's "check tool versions" guidance).
- Whether a governed eligibility-sync mechanism (HRIS/IGA integration) is
  even in scope for this repo's demos — it implies external systems this
  repo doesn't otherwise touch, and may be better left as a documented
  assumption ("bring your own eligibility source") than something this
  repo tries to demo end-to-end.
- Whether a future OpenShell release narrows the admin scope (e.g. a role
  that can provision workspaces/providers but not read other workspaces'
  sandbox data) — that would reduce this design's blast radius
  significantly and is worth checking on version bumps.

## References

- [Self-service onboarding — design notes](self-service-onboarding.md) —
  Option B, the decided direction for this demo's current scale, and the
  shared security analysis this document builds on.
