# OpenShell 0.1.4 → 0.1.8 upgrade — prep notes

Status: **live-tested against an actively-used OpenShift cluster
(sandbox341, OCP 4.22.15) on 2026-11-14.** Unlike the 0.1.2 and 0.1.4
upgrade passes, this was a genuine **in-place upgrade**, not a fresh
install: `keycloak-oidc-demo`'s `openshell` Helm release had been running
0.1.4 for 24 days with six live sandboxes underneath it (bankers'
`claude-*`/`aud-claude-*`/`aud-codex-*` sandboxes, one created the same
day). Per the user's explicit decision, all pre-existing sandboxes were
deleted and re-provisioned fresh after the Helm bump rather than preserved
— see "Sandbox survival decision" below. Branch: `v0.1.8`,
branched from `main` (not from the still-in-progress
`mlflow-tracing-claude-sandbox` branch, which this work was deliberately
isolated from via a separate git worktree).

**Cut over**: `OPENSHELL_CHART_VERSION` is now `0.1.8` in
`demos/keycloak-oidc/.env.example` and every README reference that states
it as the guide's target version, following live verification of the main
recipe (steps 1–5, including the curl-based isolation suite — 19/19
passed, identical to 0.1.4's result). Full scene-by-scene audited
walkthroughs for all three bankers were **not** completed this round — see
"Scene verification — partial, blocked on a parrot tooling limitation"
below; this was an explicit user decision to accept partial verification
rather than keep debugging TUI automation.

Sources: GitHub release notes for v0.1.5 through v0.1.8 (fetched via the
unauthenticated GitHub API), `helm show values`/`helm template` diffed
against 0.1.4 (including reading the actual chart source for anything
ambiguous — see the `key-encryption-key` finding below), direct inspection
of this repo's provider profiles/policies/scripts, and live testing
against the cluster, including recovering from a real incident caused
during this session's own Helm upgrade attempt.

## Incident: the first upgrade attempt broke the gateway for ~6 minutes

**Root cause: operator error, not an OpenShell bug.** The first
`helm upgrade --install openshell ... --version 0.1.8 -f
helm/values-certmanager.yaml` command run this session omitted the `--set`
overrides the demo's actual documented install process requires.
`helm/values-certmanager.yaml` ships with a literal `<keycloak-host>`
placeholder in `server.oidc.issuer` — the README's "2a. Helm install" /
"Install command" section fills this in via `--set
"server.oidc.issuer=https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}"`
(plus `openshiftRoute.host`, cert SAN overrides, and conditionally
`server.oidc.caConfigMapName`) at install time — never by editing the
values file itself. Running `-f values-certmanager.yaml` alone put the
literal placeholder string into the live config, and the gateway
immediately crash-looped:

```
Error:   × configuration error: OIDC initialization failed: OIDC issuer must be an
  │ absolute URL: invalid international domain name
```

**Recovery was not a simple rollback.** While diagnosing this, the brief
0.1.8 gateway process had already run far enough to apply a database
migration before crashing on the OIDC config check. Reverting the release
back to chart 0.1.4 (to restore service with known-good config) then
failed with a *different* error:

```
Error:   × execution error: migration error: migration 9 was previously applied but
  │ is missing in the resolved migrations
```

i.e. the database was now schema-ahead of what the 0.1.4 binary
understands. **There was no way back — the only path to a healthy gateway
was forward**, to 0.1.8 with the corrected `--set` flags (the exact
command from the README's "Install command" section, re-derived from
`.env`). Total user-facing downtime, first broken apply to healthy
`Running` pod: approximately 6 minutes (14:51–14:57 CET). One additional
wrinkle: after the corrected `helm upgrade`, the StatefulSet's rolling
update did not automatically recreate the crash-looping pod with the new
template (`currentRevision`/`updateRevision` mismatch persisted) — an
explicit `oc delete pod openshell-0` was needed to force it onto the
correct revision, twice (once after the accidental 0.1.4 revert, once
after the corrected 0.1.8 apply).

**Takeaway for future upgrades of this demo:** never run `helm
upgrade --install` against `values-certmanager.yaml` with only `-f` — the
full install command with its `--set` overrides (README "2a. Helm
install") is not optional boilerplate, it's load-bearing. This mistake is
entirely this session's own, not a 0.1.8 regression; `demos/base`'s and
earlier `keycloak-oidc` upgrade passes never hit it because they were
fresh installs where this was more front-of-mind.

**Confirmed positive finding from the incident:** once the gateway was
healthy, its logs showed all six pre-existing (0.1.4-era) sandboxes
reconnecting and successfully reconciling against the new 0.1.8 gateway —
`OCSF AUTH:LOGON ALLOWED` for every supervisor, provider credential
refreshes completing, `GetSandboxProviderEnvironment`/
`ReportSandboxConfiguration` calls succeeding. This is live evidence
supporting v0.1.6's own release-note claim (see below) that existing
sandboxes survive a gateway version bump without disruption — at least for
the few minutes before this session deleted them anyway per the sandbox
survival decision.

## Sandbox survival decision

Before running the live upgrade, the user was asked whether the six
pre-existing sandboxes (including one created same-day by the separate
`mlflow-tracing-claude-sandbox` branch's work) needed to survive. Decision:
**no — recreate everything from scratch to verify the upgrade works end to
end**, rather than trying to preserve them. All six were deleted
post-upgrade (`openshell sandbox delete <name> --workspace <ws>` per
sandbox) and nine fresh ones re-provisioned: `claude-alice`/`claude-bob`/
`claude-charlie` (plain, gateway-routed, via `15-provision-claude-sandbox.sh
--gw`) plus `aud-claude-*`/`aud-codex-*` for all three bankers (via
`17-provision-scene7-both-agents.sh --gw`). All nine reached `Ready`
cleanly on the first attempt, no workarounds needed — confirming none of
the 0.1.2/0.1.4-era provisioning workarounds (the image `chmod -R a+rX`
fix, the `--policy`-at-creation race fix) have regressed.

## What changed 0.1.5 → 0.1.8, and what it means for this demo

### Helm chart — backward-compatible, additive only

Diffed `helm show values` 0.1.4 → 0.1.8 (937 vs 797 lines) and rendered
both `helm/values.yaml`/`values-certmanager.yaml` against the real 0.1.8
chart (`helm template --set agentSandbox.preflight.enabled=false`, offline
rendering only — the Agent Sandbox CRD preflight check still only passes
under a real `helm upgrade` with live API discovery, unchanged since
0.1.2). Every value this demo sets is unaffected. Concretely:

- **New opt-in keys, all unused here:** `supervisor.*` (shared-supervisor
  pool — this demo never sets `supervisor_topology = "gateway"`),
  `server.extraEnv`, `pkiInitJob.podLabels`/`podAnnotations`,
  `priorityClassName`, `topologySpreadConstraints`, `metrics.tls.*`.
- **`server.ocsfLog.schemaVersion`** (v0.1.7 deprecates it wholesale,
  native OCSF 1.8.0 only now) — never set anywhere in this repo. The only
  hit for `ocsf_schema_version`/`OPENSHELL_OTLP_OCSF_SCHEMA_VERSION` is the
  README's verbatim echo of `openshell settings set --help`'s allowed-keys
  list, unrelated to this Helm value. No warning fires.
- **Default security posture tightened, non-breaking:** the gateway
  container and the certgen (PKI init job) init container both now get
  `seccompProfile: RuntimeDefault` by default; certgen also gets
  `runAsNonRoot: true` and drops all capabilities. Confirmed live: the
  certgen job and gateway pod both came up clean under these tightened
  defaults with no values-file changes.
- **New ephemeral `gateway-state` volume** (`XDG_STATE_HOME=
  /var/lib/openshell-state`, `emptyDir`) — "the gateway stages normalized,
  content-addressed supervisor trust artifacts" there per the rendered
  template's own comment. No PVC, no persistence concern.
- **The `key-encryption-key` Secret's value differs between separate
  `helm template` runs — this is template-render noise, not a real risk,
  confirmed by reading the chart source.** The generating template
  (`templates/credential-storage-key-encryption-key-secret.yaml`) does
  `lookup "v1" "Secret" .Release.Namespace $secretName` and reuses the
  existing value whenever a live cluster has one; it only generates a
  fresh random key when none exists. `helm template` has no live API
  access, so `lookup` always misses there, producing a new random value on
  every offline render regardless of version — explaining the diff without
  it being a real behavior change. The Secret also carries
  `helm.sh/resource-policy: keep`. Confirmed in practice: the real `helm
  upgrade` against the live namespace did not invalidate any already-
  imported provider credentials.

### v0.1.8 breaking change — static provider credential placement (PR #4886) — confirmed non-issue

"Static provider credentials without placement metadata now resolve only
in the `Authorization` header or through explicitly enabled body/WebSocket
rewrite policies... custom headers such as `x-api-key`... need an explicit
placement declaration." Checked every credential entry in every file under
`demos/keycloak-oidc/providers/*.yaml`: all eight live profiles
(`byo-claude`, `byo-codex`, `deepseek-claude`, `deepseek-codex`,
`session-auditor-anthropic`, `session-auditor-openai`, `user-refresh`,
plus the unused draft `token-exchange`) declare an explicit `auth_style`
(`bearer` or `header`) on every credential — including the two that use a
non-`Authorization` header (`deepseek-claude`, `session-auditor-anthropic`,
both `auth_style: header` with `x-api-key`, already explicit, never
implicit). No query-param or path-slot placements anywhere. The one file
with no `auth_style` (`token-exchange-profile.yaml`) is explicitly marked
`DRAFT ONLY, schema unconfirmed` and isn't imported anywhere — flagged for
whoever finishes that draft, not a blocker now.

### v0.1.8 — OCSF sandbox attribution field move (PR #5048) and v0.1.7's native-1.8.0-only change (PR #5036) — confirmed non-issues

This demo's audit pipeline (`session-auditor` → `audit-collector` OTLP
pipeline → `audit-tempo`/Prometheus → `audit-dashboard`) is entirely
self-contained: `session-auditor` builds its own span/metric attributes
directly in `util/session-auditor/src/main.rs`, never consuming the
gateway's native OCSF audit-log/OTLP-OCSF stream at all. Grepped
`audit-dashboard` and `session-auditor` source for `container.uid`,
`container.name`, `unmapped.*`, OCSF resource-type fields, and
`ocsf_schema_version`/`schema_version` — zero hits anywhere except the
historical upgrade docs and a verbatim CLI-help echo. Both changes are
real for anyone wiring a SIEM or collector into the gateway's own OCSF
emission; this demo never does.

### v0.1.6 breaking change — gateway auth requirement (PR #4821) — confirmed non-issue

Gateways now refuse to start unless OIDC, mTLS, or gateway JWTs are
configured (or `allow_unauthenticated_users = true` is explicitly set).
`helm/values-certmanager.yaml`'s `server.auth.allowUnauthenticatedUsers:
false` plus full `server.oidc.*` configuration already satisfies this —
confirmed live, the gateway starts and authenticates exactly as before.

### v0.1.6 — sandbox control-connection direction change (PR #4656) — real finding, reported honestly rather than assumed

"New sandboxes... open the control connection from the sandbox to its
supervisor... Upgrade the supervisor and sandbox runtime binaries
together, because older binaries reject the new sandboxes." This is about
the **Kubernetes-driver-internal supervisor↔gateway control channel**,
architecturally distinct from the CLI/SDK↔gateway public API that
`util/parrot`'s pinned SDK talks to (the connection
`docs/openshell-networking.md` actually documents) — those are two
separate concerns and shouldn't be conflated.

This repo's own code and docs say nothing about the supervisor-internal
control channel, so rather than assume the changelog's own reassurance
("running sandboxes keep their existing connection direction") holds
without checking, this was verified live: during the incident above, all
six pre-existing 0.1.4-era supervisors reconnected and kept working
against the new 0.1.8 gateway with no errors, confirming the claim in
practice for this cluster. They were deleted minutes later anyway (see
"Sandbox survival decision"), so this wasn't soak-tested over a longer
window, but the immediate-reconnect case is confirmed, not assumed.

### Everything else from the four changelogs — confirmed non-issues or pure improvements

- **WebSocket 403 (v0.1.8, PR #5336) and symlink-policy (v0.1.8, PR
  #5278):** grepped `demos/keycloak-oidc/policies/` — every
  `network_policies` entry uses `protocol: rest`, zero `websocket`
  references; no filesystem-policy path is a symlink under a writable
  workspace mount. Non-issues.
- **Deleted-sandbox gateway memory leak fix (v0.1.8, #5428) and stream-
  buffer pre-allocation fix (#5408):** pure reliability improvements,
  directly relevant to *why* this upgrade is worth doing on a long-lived
  gateway process (the 0.1.4 gateway pod was 24 days old before this
  upgrade).
- **PostgreSQL-unavailable fast-fail (#5244) and faster Kubernetes sandbox
  deletion with no 30s grace-period wait (#5279):** pure improvements, no
  demo-side action.
- **Provider-attachment credential-inspection timing fix (#5310,
  "checks against the sandbox's latest saved policy instead of its
  creation policy"):** plausibly related to the `--policy`-at-creation
  workaround this demo's scripts 14/15/16 already use (documented in
  `docs/openshell-0.1.2-upgrade.md`'s "Second blocking finding") — but
  since this demo already passes policy at creation time, the distinction
  this PR fixes may not even apply to this demo's call pattern. Worth
  re-checking if that workaround is ever revisited, not acted on here.
- **`openshell status --gateway <name>` exit-code change (#5379):**
  grepped `demos/keycloak-oidc/scripts/*.sh` for `status --gateway` usage
  — zero hits, only a prose mention in the README. Non-issue.
- Sandbox suspend/resume (#4765), shared-supervisor pool (#4695 and
  friends), metrics TLS listener (#3877), `service_hostname_format =
  "hashed"` (#4842), OIDC client-credentials token renewal (#4389), OTLP
  agent-trace relay and `additional_ca_bundle` (#4276/#5083/#5084/#3292) —
  all new, all opt-in, none referenced by this demo's current
  configuration. The `additional_ca_bundle` feature is exactly what the
  separate `mlflow-tracing-claude-sandbox` branch's MLflow work needs (its
  root-caused blocker is the supervisor not trusting RHOAI's MLflow
  certificate on its backend leg) — confirmed present and available in
  this 0.1.8 chart, but wiring it up is explicitly that branch's work, not
  this upgrade's.

## `util/parrot` — SDK bumped, one compile fix, new release cut

`make bump-openshell-sdk OPENSHELL_SDK_TAG=v0.1.8` hit the same GNU-vs-BSD
`sed -E` incompatibility this repo's own upgrade docs have hit before (the
Makefile's substitution pattern needs GNU sed; ran it manually via `gsed`
instead of patching the Makefile). One compile error followed, matching a
real v0.1.8 API change: `OidcTokenBundle` gained a `client_credentials:
bool` field (v0.1.8 PR #4830, "whether the token came from the
`client_credentials` grant, so it can be renewed by repeating that
grant"). Fixed in `parrot-core/src/auth.rs`'s `StoredOidcRefresher::
refresh` by carrying the field forward from the loaded bundle, the same
way `issuer`/`client_id` already are — this refresher only ever rotates a
refresh token, never a client-credentials grant, so just preserving
whatever the stored bundle already said is correct.

`parrot-core`'s 39 tests and `parrot-tui` both build and pass clean against
the new SDK. **`parrot-gpui` fails to build on this x86_64 macOS host** —
`bindgen`/Clang can't resolve `CoreMedia/CMFormatDescription.h` and hits an
internal pointer-width assertion. Confirmed via `git diff Cargo.lock` that
`gpui`'s own pinned version is completely untouched by this SDK bump, and
`ci-parrot.yml` only ever tests `macos-latest` (Apple Silicon) — this is a
pre-existing Intel-Mac toolchain gap, never previously exercised, not a
regression from this upgrade. `cargo check -p parrot-gpui` type-checks
without the native build succeeding further than this, per the repo's own
Makefile comment about what that command is good for on Linux without
`libxcb`/`libxkbcommon`.

Cut `parrot-v0.1.3` via `make release-patch` (cargo-release): version
bump, commit, tag, push, CI release workflow (`ci-parrot.yml`) completed
successfully and published binaries. README's pinned install tag updated
from `parrot-v0.1.1` to `parrot-v0.1.3`.

**Scene-by-scene UI verification (`./scripts/19-run-scene7-persona-demo.sh
--aud <user>`, both `--ui`/gpui and default parrot-tui) was not completed
end to end** — see the next section. `parrot --version` and real,
correctly-behaving agent turns (multi-step MCP tool-calling, correct
refusals, error recovery) were confirmed against the live 0.1.8 gateway
for all three bankers' first couple of turns before the automation
limitation below was hit.

## Scene verification — partial, blocked on a parrot tooling limitation

Per AGENTS.md §4, the main recipe (steps 1–5 including
`scripts/08-verify-isolation.sh`) was fully re-verified: **19/19 isolation
checks passed**, identical to the 0.1.4 result (all three bankers reach
their four shared servers; only Alice reaches `mcp-compatibility`; Bob's
four cross-tenant probes against Alice's and Charlie's clients are all
denied, no data leaked).

The user additionally asked for full audited scene coverage across all
three bankers and both agents (their own documented workflow: `source
scripts/as.sh <user>` + `./scripts/19-run-scene7-persona-demo.sh --aud
<user>`). This surfaced a real automation limitation, not a 0.1.8
regression:

**`expect`'s pty defaults to a 0×0 window size** when `expect`'s own
controlling terminal isn't a real tty (true in this sandboxed shell
environment) — `parrot-tui`'s ratatui/crossterm renderer then spins
forever redrawing into a zero-area screen instead of erroring, producing
an endless stream of cursor-hide/reset-color escape codes with no visible
content. Fixed for the *first* turn by setting `stty_init "rows 50 columns
200"` before `spawn` — confirmed this unblocks real agent activity (tool
calls, reasoning, correct refusals, all visible in the captured
transcript). **The same hang recurs after the second turn**, reproducibly,
across all three bankers, regardless of whether the three runs are
executed concurrently or one at a time (ruling out LLM-backend contention
as the cause) — root cause not identified within this session's time
budget. This looks like a `parrot-tui`/crossterm pty-handling issue
specific to non-interactive automation, not an OpenShell-side problem:
`util/parrot`'s own README/script-19 comments already warn "Run this from
an actual interactive terminal (not backgrounded/piped/non-interactively)"
— this session's attempt to do exactly that turned out to confirm the
warning rather than disprove it.

**Confirmed working, before the automation hang, for all three bankers'
audited `claude` sandboxes:** real multi-step MCP tool-calling (e.g. Bob's
Scene 1 correctly chained `get_upcoming_meetings` → `get_meeting_notes` →
`get_positions` → retried `get_performance` across periods after a `YTD`
error → scoped `get_relevant_news`), correct refusal-without-fabrication
behavior (Alice's Scene 6 Part 1 correctly reported a client not in her
book rather than inventing one), and no tool-call or MCP-server errors
beyond an expected, gracefully-handled `no hay datos de rendimiento para
el periodo 'YTD'` from one server.

**Per explicit user decision, this was accepted as sufficient partial
verification** rather than continuing to debug `parrot-tui`'s automation
behavior — the user will run any remaining scenes interactively at their
own convenience, which is parrot's actually-supported mode of use. Full
scene-by-scene audited walkthroughs for every banker/agent combination
remain untested on 0.1.8; this is a known gap, not a silent skip.

## `util/onboarding-web` — rebuilt, redeployed, basic health confirmed

Not called out in the original task scope, but surfaced during research:
`util/onboarding-web/Containerfile` hard-pins `ARG OPENSHELL_VERSION` to
download and bake in a specific `openshell` CLI release — the Containerfile's
own comment documents a real prior live failure (2026-10-11, CLI 0.0.106
baked in against a 0.1.2 gateway) with a protobuf `buffer underflow`
decoding `ListProvidersRequest`. This is deployed live in
`keycloak-oidc-demo` right now (the `onboarding-web` Helm release) — same
risk class as `util/parrot`'s SDK pin, just a baked CLI binary instead of a
compiled SDK dependency.

Bumped `ARG OPENSHELL_VERSION` to `0.1.8` and `Cargo.toml`'s version to
`0.1.2` (not a Containerfile-only rebuild reusing the existing `0.1.1`
tag) **specifically to get a new image tag** — the deployment's
`imagePullPolicy` is `IfNotPresent`, so reusing `0.1.1` risked a node
serving its already-cached stale image after the push instead of pulling
the rebuilt one. Built and pushed via the `carlos-kenshin` remote Linux
podman host (this repo's established pattern for images needing a Linux
builder — see memory/this repo's own convention): confirmed via `podman
run --rm --entrypoint openshell quay.io/atarazana/onboarding-web:0.1.2
--version` → `openshell 0.1.8`. Redeployed the Helm release pointing at
the new tag; pod reached `Ready`, route returns `HTTP 200`.

**Not independently re-verified this round:** an actual banker OAuth login
through Keycloak all the way to a working provider attachment (the exact
scenario the protobuf-mismatch failure mode above describes). The
README's step 3.1a warning and `TODO.md`'s tracking item — both stale even
before this session, since an interim rebuild had already fixed this for
0.1.4 without the docs being updated — are corrected to reflect the
current (0.1.8) state and flag this specific gap explicitly rather than
asserting either way.

## Remaining work

1. **Full scene-by-scene audited verification** for all three bankers,
   both agents, against 0.1.8 — explicitly deferred per user decision, see
   above. The user will run these interactively.
2. **`parrot-tui`'s automation hang after the second turn** — not
   root-caused. Worth revisiting if headless scene-demo automation is
   wanted again in the future; candidates not yet tried: driving via
   `tmux` instead of `expect` (not installed on this host), or bypassing
   parrot entirely for automation by replicating its underlying
   `openshell sandbox exec ... --env ... -- claude/codex` calls directly
   (fully scriptable, no TUI/pty involved at all).
3. **`onboarding-web`'s actual login flow** — rebuilt and redeployed, but
   the live Keycloak-OAuth-to-working-provider path wasn't independently
   re-exercised this round. Flagged as [VERIFY] in the README rather than
   asserted either way.
4. **`parrot-gpui` doesn't build on x86_64 macOS** — pre-existing gap,
   never covered by upstream CI (`macos-latest` runners are Apple
   Silicon). Not blocking, but means this specific host can't locally
   verify the `--ui`/gpui frontend at all; `parrot`/`parrot-tui` (the
   default) is unaffected.
5. **`token-exchange-profile.yaml`'s missing `auth_style`** — a draft file,
   not live-imported anywhere, but worth adding explicit placement before
   it's ever used, now that v0.1.8 requires it.
6. Merging `v0.1.8` back into `main` is a separate step, not done
   here.
