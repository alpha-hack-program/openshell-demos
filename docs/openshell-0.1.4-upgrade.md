# OpenShell 0.1.2 → 0.1.4 upgrade — prep notes

Status: **live-tested against a fresh OpenShift cluster (sandbox341, OCP
4.22.15) on 2026-10-20.** This was a clean install, not an in-place
upgrade — the `keycloak-oidc-demo` namespace, its `openshell`/`mcp-servers`
Helm releases, SPIRE, the local `openshell` CLI gateway registration, and
local mTLS certs had all already been torn down
(`scripts/99-teardown.sh keep-keycloak`) before this work started. Keycloak
itself (RHBK operator, Keycloak CR, realm) was **never touched** — this is
purely an OpenShell version bump for `demos/keycloak-oidc`.
`demos/base` is explicitly **out of scope** and stays pinned at 0.0.106.
Branch: `v0.1.4-upgrade`. **Cut over**: `OPENSHELL_CHART_VERSION` is now
`0.1.4` in `demos/keycloak-oidc/.env.example` and every doc that echoes it
as the guide's target version, following live verification of the main
recipe (steps 2–5 + the curl-based isolation suite). The items in
"Remaining work" below are follow-ups that don't block this cutover — see
that section for what's still open (optional recipes untested, a
not-yet-released `onboard` fix, a pending `PLATFORM_OPENAI_*` revert).

0.1.3 is an intermediate release between 0.1.2 and 0.1.4 — both are
covered here. The gateway is confirmed **0.1.4** live
(`openshell status` → `Version: 0.1.4`).

Sources: GitHub release notes for v0.1.3/v0.1.4 and the individual PRs
they reference (fetched via the unauthenticated GitHub API —
`gh auth` wasn't available in this session), `helm show values
oci://ghcr.io/nvidia/openshell/helm-chart --version 0.1.4` diffed against
0.1.2, `helm template` against this demo's actual `values.yaml`/
`values-certmanager.yaml`, the locally-installed 0.1.4 CLI's own `--help`
output, and live testing against the cluster.

## What changed 0.1.2 → 0.1.4, and what it means for this demo

### Helm chart — backward-compatible, no values-file changes needed

Rendered both `helm/values.yaml` and `helm/values-certmanager.yaml`
against the real 0.1.4 chart (`helm template`, read-only, no cluster
contact) before touching anything live. Every key this demo already sets
(`podSecurityContext.fsGroup`, `securityContext.runAsUser`,
`server.auth.allowUnauthenticatedUsers`, `server.oidc.*`,
`server.providerTokenGrants.spiffe`, `openshiftRoute.*`, `certManager.*`)
is unchanged in 0.1.4. New additions are all opt-in and unused here:

- `rbac.create`/`rbac.clusterScoped.*` (default `true`/`true` — matches
  existing behavior).
- `autoscaling.*` (default `enabled: false`).
- `server.gatewayConfig`/`server.ocsfLog.*` (schema-v2 TOML config /
  native OCSF JSONL — neither referenced by this demo).
- `credentialDrivers.vault.caConfigMapName` (not used — no Vault driver
  here).
- **`server.sandboxUid`/`server.sandboxGid`** (new in 0.1.3, closes
  upstream issue #2697) — see below, this does **not** supersede the
  existing image-rebuild workaround.
- `replicaRouting.*` (HA feature, single-replica demo, not applicable).

Confirmed live: `helm upgrade --install openshell
oci://ghcr.io/nvidia/openshell/helm-chart --version 0.1.4 -f
helm/values-certmanager.yaml ...` succeeded on the **first try**, no
changes to either values file. Gateway pod came up, Route created, OIDC
login worked unchanged.

### The 0.1.2-era GID/SCC bug is unaffected by the new `sandboxUid`/`sandboxGid` Helm values — workaround still required

PR [#3947](https://github.com/NVIDIA/OpenShell/pull/3947) ("feat(helm):
add sandbox UID and GID values", merged 2026-10-02, in 0.1.3) looked at
first glance like it might close the gap documented in
[`openshell-0.1.2-upgrade.md`](openshell-0.1.2-upgrade.md)'s "Blocking
finding" (OpenShift forces `runAsGroup` to the same namespace-derived
value as `runAsUser`, which isn't `0`, breaking any image that relies on
OpenShift's own "arbitrary UID + group 0" convention for pre-seeded
`/sandbox` content). It doesn't: the new `sandboxGid` Helm value still
validates to the range **1..4294967294** — `0` is rejected exactly as
before, so there is still no way to configure the group OpenShift's own
convention actually needs. **`quay.io/atarazana/{claude,codex}-sandbox:0.4.0`
(the `chmod -R a+rX /sandbox` rebuilds) remain required** — confirmed live,
all three bankers' `claude-<id>` sandboxes reached `Ready` unmodified with
these images.

Separately, PR [#4054](https://github.com/NVIDIA/OpenShell/pull/4054)
("fix(kubernetes): wait for OpenShift SCC annotations", merged 2026-10-05,
also in 0.1.3) changes annotation-copying/timing for a different,
newer feature — auto-provisioned **managed workspace namespaces** (one
namespace per workspace, SCC annotations awaited rather than copied from
the gateway namespace). This demo uses a single pre-created namespace
(`workspaceResources.enabled: true`, the chart's existing in-namespace
mode), not managed workspace namespaces, so this PR doesn't apply here —
confirmed no related behavior change during live testing.

### `sandbox create --upload` + Git filtering (0.1.3, breaking) — not a blocker here

PR [#3957](https://github.com/NVIDIA/OpenShell/pull/3957)
("fix(cli)!: stop uploads when Git filtering fails or selects no files",
merged 2026-10-02) makes `sandbox create --upload`/`sandbox upload` hard-fail
if Git filtering errors or selects zero files, instead of silently
uploading unfiltered. Checked both `--upload` call sites in this repo
(`scripts/14-provision-codex-sandbox.sh`, `scripts/15-provision-claude-sandbox.sh`):
both source from `mktemp` files under `$TMPDIR`, outside any Git work
tree, so the new behavior only adds an informational warning —
confirmed live:

```
Warning: /var/folders/.../tmp.XXXXXX.json is outside a Git work tree;
uploading without Git filtering (.gitignore rules are not applied).
```

— not fatal, no script changes needed.

### `feat(providers): support stable credential placeholders` (0.1.4) — not adopted, candidate follow-up

PR [#3339](https://github.com/NVIDIA/OpenShell/pull/3339), merged
2026-10-12 (0.1.4), adds an opt-in `credentials[].stable_placeholder:
true` field to provider profiles: once set, rotating the underlying
credential keeps an already-baked-in placeholder reference valid instead
of invalidating it. This looks like a direct fix for the "Operational
gotcha: `provider profile update` invalidates a baked MCP config" issue
documented in the 0.1.2 doc (rotating `user-<id>`'s attachment currently
requires deleting and re-provisioning the sandbox to pick up the new
placeholder). **Not adopted in this pass** — this upgrade is scoped to the
version bump itself, not new functionality. Worth a follow-up to add
`stable_placeholder: true` to `providers/user-refresh-profile.yaml` and
confirm it actually closes that gotcha.

### Confirmed non-issues

- `fix(kubernetes): refresh sandbox runtime image on start` (0.1.4,
  PR #4376) — doesn't change anything this demo depends on; not observed
  during testing.
- No changelog entry in either 0.1.3 or 0.1.4 mentions the 0.1.2-era
  sandbox-create-vs-fail-closed-rollback race
  ([#3145](https://github.com/NVIDIA/OpenShell/issues/3145)/[#3758](https://github.com/NVIDIA/OpenShell/issues/3758)).
  **The `--policy`-at-creation workaround in `scripts/14`/`scripts/15`
  remains required** — confirmed live, all three bankers' sandboxes
  reached `Ready` immediately via `sandbox create --policy ... --upload
  ... --detach`, same as on 0.1.2.
- `feat(helm): migrate gateway configuration to gatewayConfig` (0.1.3,
  PR #3384) — confirmed backward-compatible by its own PR description
  (legacy value aliases retained) and by `helm template` rendering
  cleanly against both of this demo's values files with no changes.
- `feat(sandbox): add main restart policy` (0.1.3, PR #2798) — new
  `--restart-policy`/`server.sandboxRestartPolicy` surface, unused by
  this demo's scripts. See the "pod-delete no longer self-heals" finding
  below for a related, less convenient consequence of this feature's
  default (`never`).
- The `agent-sandbox-preflight.yaml` live-cluster check
  (`agents.x-k8s.io/v1beta1` or `v1alpha1` must be served) is unchanged
  since 0.1.2, not new in 0.1.3/0.1.4. It only fails under **offline**
  `helm template` (no live API discovery) — a real `helm install`/
  `upgrade` against this cluster passes, since the CRD
  (`sandboxes.agents.x-k8s.io` v1beta1) and the `agent-sandbox-system`
  namespace are both live. Confirmed by rendering locally with
  `--set agentSandbox.preflight.enabled=false` for offline validation only
  (never passed to the real install).
- `openshell policy update --help`, `settings set --help`,
  `provider`/`gateway` subcommand structure — all unchanged from what the
  0.1.2 doc already catalogued. `settings set --global --key
  providers_v2_enabled` is still an unknown-key error (Providers v2
  remains always-on) — step 2d stays skipped.
- Cluster-wide MCP Gateway infra (RHCL operator, Kuadrant CR, the
  `mcp-gateway` Gateway/GatewayClass/MCPGatewayExtension in
  `mcp-gateway-system`) — unaffected by this demo's own namespace
  teardown (it's cluster-shared, owned by a separate `llm-d-guide` chart
  per the most recent commit on `main`), confirmed still `Ready: True`,
  18 days old. Step 4a was a verify-only step, not a reinstall.

## New findings on 0.1.4 (environment-specific, not version-bump regressions)

Three of the four issues below are **macOS-only environment gaps** on the
machine this session ran from (bash 3.2, BSD `sed`/`mktemp`) — this demo's
scripts have evidently only ever been live-tested from Linux/Fedora
toolbox before, despite the README's own
[Troubleshooting](../demos/keycloak-oidc/README.md#d-troubleshooting)
table claiming all three were "fixed in-repo as of 2026-10-11". **They
weren't** — all three were still present, unmodified, in `scripts/06`/
`14`/`15`/`16` on this branch. Diagnosed first with session-local
workarounds (`brew install gnu-sed` prepended to `PATH`, and exported
`mapfile`/`mktemp --suffix=` shim functions — bash allows overriding
builtins with same-named exported functions, which propagate to child
`#!/usr/bin/env bash` invocations), then **fixed for real in the scripts
themselves**, matching the exact patterns the README's own
(now-corrected) Troubleshooting table already specified, and reverified
against plain BSD tools with no shims/overrides active:

1. **`scripts/06-deploy-mcp-servers.sh:116` used `mapfile -t`**, a bash-4+
   builtin absent from macOS's system bash (3.2, GPL-licensing-frozen).
   Replaced with a portable `while IFS= read -r` loop.
2. **`scripts/15-provision-claude-sandbox.sh`/`14`/`16` used `mktemp
   --suffix=...`**, a GNU coreutils flag BSD `mktemp` doesn't support.
   Replaced with `"$(mktemp -d)/<name>.<ext>"` at all six call sites.
3. **`scripts/15-provision-claude-sandbox.sh:131` and
   `scripts/16-provision-audited-sandbox.sh:162` used `sed
   's|https\?://||'`** — `\?` is a GNU regex extension; BSD `sed` doesn't
   expand it, so the pattern fails to strip the scheme and
   `LLM_HOST`/`AUDITOR_LLM_HOST` end up as the literal string `"https"`,
   breaking the rendered provider profile (`host: "https"`) and producing
   a confusing gateway-side error (see next finding) rather than a clear
   one. Replaced with the portable `sed -E 's|^https?://||; s|/.*||'`.

The README's Troubleshooting table and its "Confirmed on macOS" callout
near the top of the guide are both corrected to reflect this (the false
"fixed in-repo as of 2026-10-11" claim is now dated 2026-10-20, with a
note explaining the discrepancy).

### Real bug (not macOS-specific): `util/onboard` never substitutes `<mcp-gateway-host>`

This one **is** a genuine functional gap, independent of OS, and worth
fixing or at least tracking — not found by the 0.1.2 effort because that
round's Scenes 1–6 ran against **direct** MCP access; `--gw` was only
validated for Scene 4d separately, against a profile that (per that doc)
had its `<mcp-gateway-host>` endpoint added to
`providers/user-refresh-profile.yaml` — but the doc doesn't show the
`onboard` tool itself being exercised in that `--gw` pass, so this gap may
simply never have been hit end-to-end before.

`util/onboard/src/main.rs` (confirmed by reading the source) substitutes
exactly two placeholders in a provider profile before importing it:
`<keycloak-host>` (always) and `<openshell-namespace>` (if
`--namespace`/`OPENSHELL_NAMESPACE` is set) — see lines ~391–412. It has
**no handling at all** for `<mcp-gateway-host>`, the placeholder
`providers/user-refresh-profile.yaml` now carries specifically for `--gw`
mode (added during the 0.1.2 effort's Scene 4d fix). Onboarding any
banker with `onboard --profile providers/user-refresh-profile.yaml`
therefore always leaves that endpoint as the literal string
`<mcp-gateway-host>` in the imported `user-scoped-api` profile.

**Symptom, confirmed live:** every `--gw`-provisioned sandbox's MCP calls
fail with the gateway's credential proxy refusing to inject the token:

```json
{"error":"credential_endpoint_mismatch","message":"Credential is not authorized for this request endpoint"}
```

HTTP 403, at the raw MCP `initialize` call — and inside Claude Code this
surfaces as something much more confusing: with no live tool connection,
the agent silently falls back to its built-in `Bash`/`Read`/etc. tools (or,
with `--tools ""` disabling those too, emits raw unparseable
`<｜DSML｜>` tool-call syntax as literal text) instead of a clean "MCP
server unreachable" error. This cost real debugging time — the first
instinct was to suspect a 0.1.4 regression or a flaky model, not a
3-line gap in a separate binary's placeholder substitution.

**Workaround applied for this test run** (not committed as a permanent
fix — `util/onboard` is a separately-released binary, out of this
upgrade's scope): after onboarding each banker, patch their already-
imported `user-scoped-api` profile directly, the same `export`/edit/
`provider profile update` pattern the README already documents for
`byo-claude`:

```bash
MCP_GATEWAY_HOST="mcp.${CLUSTER_APPS_DOMAIN}"
for USER_ID in alice bob charlie; do
  openshell provider profile export user-scoped-api --workspace "$USER_ID" -o yaml \
    > "/tmp/user-scoped-api-${USER_ID}.yaml"
  sed -i "s|<mcp-gateway-host>|${MCP_GATEWAY_HOST}|" "/tmp/user-scoped-api-${USER_ID}.yaml"
  openshell provider profile update user-scoped-api \
    -f "/tmp/user-scoped-api-${USER_ID}.yaml" --workspace "$USER_ID"
done
```

Confirmed this alone wasn't enough to reach an already-provisioned
sandbox — see the next finding.

**Fixed in this pass**: `util/onboard/src/main.rs` now accepts
`--mcp-gateway-host`/`MCP_GATEWAY_HOST` (mirroring the existing
`--namespace`/`OPENSHELL_NAMESPACE` pattern) and substitutes
`<mcp-gateway-host>` the same way, with the same "still contains the
placeholder" warning if omitted. `cargo build` confirmed clean. **Not yet
released**: this only fixes the source — the README's step 3.1b still
points at the pre-built `onboard-v0.1.3` binary, which predates this fix
and won't have the new flag. Cutting a new `onboard-v*` release (and then
updating the README's onboard command to pass
`--mcp-gateway-host "mcp.${CLUSTER_APPS_DOMAIN}"` plus bumping the pinned
tag) is tracked in `TODO.md` but not done here — until that release
ships, `--gw` onboarding still needs the manual `provider profile update`
workaround above, or building `onboard` from source
(`cargo build --release -p onboard` in `util/onboard`) instead of using
the pinned release binary.

### `oc delete pod` no longer reliably recovers a sandbox after a profile update (likely a consequence of 0.1.3's new restart-policy feature)

The README's "Editing an already-imported profile" section offers
`oc -n "$OPENSHELL_NAMESPACE" delete pod "${USER_ID}--claude-${USER_ID}"`
as the first thing to try, with a full `sandbox delete` +
re-provision only "if that alone doesn't clear it — confirmed needed at
least once". On this 0.1.4 test, deleting the pod for all three bankers'
sandboxes (to pick up the `<mcp-gateway-host>` fix above) left **every
one** in a permanent `Error`/`PodFailed` phase with no pod recreated —
`sandbox get -o yaml` showed `restart_policy: never` (the new field from
0.1.3's PR #2798, "add main restart policy") and a `Finished: True`
condition. Nothing resurrected it; this looks like the explicit, intended
behavior of `restart_policy: never` rather than a bug, but it makes the
pod-delete shortcut markedly less useful on 0.1.3+ than it evidently was
before that feature existed. Full `sandbox delete` + re-run of
`scripts/15-provision-claude-sandbox.sh` was required and worked cleanly
for all three bankers. Worth updating that README section to lead with
"delete and re-provision" rather than presenting pod-delete as the
usually-sufficient first attempt, now that a plain `never`-restart-policy
sandbox won't come back from either kind of pod removal.

## Live-verified on 0.1.4 (sandbox341, fresh install, 2026-10-20)

- **Step 2a (Helm install)** — `helm upgrade --install openshell
  oci://ghcr.io/nvidia/openshell/helm-chart --version 0.1.4 -f
  helm/values-certmanager.yaml ...` succeeded first try, unmodified
  values file. Gateway `Running`, Route created.
- **Step 2b/2c (`gateway add` + OIDC login)** — worked unchanged,
  Playwright-driven headless login as `openshell-admin`. `openshell
  status` confirms `Version: 0.1.4`, `openshell whoami` confirms
  `Name: openshell-admin`.
- **Step 2d (Providers v2)** — confirmed still obsolete/always-on, same
  `unknown setting key 'providers_v2_enabled'` error as 0.1.2. Skipped
  per the README.
- **Step 3 (workspace creation + `onboard` tool, all three bankers)** —
  workspaces created and populated via direct Keycloak admin API lookups
  (same commands as the README's manual step 3.0). `onboard --no-browser`
  driven headlessly via Playwright for alice/bob/charlie; all three
  reported "onboarded successfully" and show `STATUS: refreshed` on
  `provider refresh status`. (Step 3.1a, `onboarding-web`, was skipped per
  the README's own warning that it's currently broken on this version
  line — not re-tested here, no evidence that changed.)
- **Step 4a (MCP Gateway infra)** — verified already `Ready: True`
  (18 days old, predates this test), not reinstalled.
- **Step 4b (`scripts/06-deploy-mcp-servers.sh`,
  `MCP_GATEWAY_ENABLED=true MCP_GATEWAY_AUTH_ENABLED=true`)** — all five
  servers reached `Running`. Hit the documented embeddings cold-start
  CrashLoopBackOff for `mcp-market-news`/`mcp-kyc-compliance` — **separate
  from that**, `mcp-market-news`'s init container also failed against
  `PLATFORM_OPENAI_MODEL=publishers/llm-d-demo/models/alibaba/qwen3-8b`
  with `no healthy upstream` (503) — traced to a **pre-existing, unrelated
  cluster-capacity problem**: `qwen3-8b`'s KServe pods have been `Pending`
  for ~4 days cluster-wide (`0/6 nodes are available: 3 Insufficient
  nvidia.com/gpu`), confirmed via `oc describe pod`. Not caused by this
  upgrade. **Per user decision, temporarily pointed `PLATFORM_OPENAI_*` at
  the already-working `deepseek-flash` endpoint** (same MaaS host, a model
  that **is** scheduled) for this test run only — edited directly in this
  worktree's gitignored `.env`, not `.env.example`. **Revert this once
  GPU capacity frees up for `qwen3-8b`**, or decide to adopt
  `deepseek-flash` for `mcp-market-news` permanently (a separate decision
  from this version bump).
- **Step 5 (Claude Code harness, all three bankers, `--gw`)** — after
  applying the `<mcp-gateway-host>` provider-profile workaround above, all
  three (`claude-alice`, `claude-bob`, `claude-charlie`) reached `Ready`
  cleanly via `sandbox create --provider --provider --policy --upload
  --detach`, confirming the 0.1.2-era GID workaround and
  `--policy`-at-creation race workaround both still hold unmodified.
  `claude --version` → `2.1.263 (Claude Code)` in each, matching the
  0.1.2 doc's own image-verification result exactly. Token substitution
  (`__USER_ACCESS_TOKEN__` → real token) succeeded on the first attempt
  for all three.
- **Scene 1 (Bob preps for a meeting)** — passed, real multi-hop tool
  chaining (`get_upcoming_meetings` → `get_meeting_notes` →
  `get_positions`/`get_performance` → `get_relevant_news`, for two
  upcoming meetings), correct compliance-flag reasoning
  (sector-concentration rule applied to both clients), no scripted
  behavior.
- **Scene 4a (forced cross-tenant call)** — passed: `bob`'s agent, told
  explicitly not to refuse, called `get_positions` for `cli-004` (Alice's
  client) and received a genuine server-side denial —
  `MCP error -32602: client_id no encontrado para el llamante
  autenticado` — proving `assert_owns_client` tenant isolation fires for
  real through a live agent call on 0.1.4, not just via the curl script.
- **Curl-based isolation verification
  (`scripts/08-verify-isolation.sh`)** — **19/19 checks passed**: all
  three bankers reach their four shared servers (200), only Alice reaches
  `mcp-compatibility` (Bob/Charlie get 403), and Bob's four cross-tenant
  probes against Alice's and Charlie's clients (via both `mcp-portfolio`
  and `mcp-kyc-compliance`) are all denied with no data leaked.

## Remaining work

1. **Revert the `PLATFORM_OPENAI_*` → `deepseek-flash` override** in the
   real (non-worktree) `.env` once `qwen3-8b` has GPU capacity again, or
   make the swap to `deepseek-flash` permanent as a separate decision —
   not resolved here, purely a stand-in to unblock this test run.
2. **`util/onboard`'s `<mcp-gateway-host>` gap — source fixed, release not
   cut.** `cargo build` confirmed clean; cut a new `onboard-v*` release
   and update the README's step 3.1b command + pinned tag (tracked in
   `TODO.md`). Until then, `--gw` onboarding still needs either the
   manual `provider profile update` workaround above or building
   `onboard` from source directly.
3. ~~Update the README's "Editing an already-imported profile" section~~ —
   **done.** Now leads with "delete and re-provision", with a direct
   explanation of why `oc delete pod` alone doesn't recover a
   `restart_policy: never` sandbox on 0.1.3+.
4. Scenes 4b/4c/4d, 5a/5b, 6, and Scene 7 (audit trail) were **not**
   re-run in this pass — Scene 1 and 4a were chosen as the minimum
   representative set (baseline multi-hop chaining + the core
   tenant-isolation proof) given this upgrade's time budget. Worth a
   fuller pass later, same shape as the 0.1.2 doc's "Scenes 1–6" section,
   if a full re-verification is wanted.
5. **Optional recipes (Codex + BYO LLM, remaining Claude Code scenes
   4b–4d/5a/5b/6/Scene 7) — explicitly skipped for this round**, per
   `AGENTS.md` §4 (asked, user chose to skip rather than run them). Not
   tested on 0.1.4.
6. ~~Bump `OPENSHELL_CHART_VERSION` in `demos/keycloak-oidc/.env.example`
   and every doc that echoes it as the guide's target version~~ —
   **done**, following live verification of the main recipe. Items 1–2
   above are consciously deferred, not blockers to this cutover. Merging
   `v0.1.4-upgrade` back into `main` is still a separate step, not done
   here.
