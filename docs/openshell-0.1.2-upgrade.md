# OpenShell 0.0.106 → 0.1.2 upgrade — prep notes

Status: **live-tested against a fresh OpenShift cluster (sandbox341,
OCP 4.22.15) on 2026-10-01/02.** Part I steps 1–5 (Keycloak, gateway
install, OIDC login, Providers v2 onboarding, MCP servers, Claude Code
sandbox provisioning for all three bankers) pass on 0.1.2 with the fixes
below. `quay.io/atarazana/claude-sandbox:0.4.0` and
`quay.io/atarazana/codex-sandbox:0.4.0` are built, pushed, and public.
**Scenes 1–6 (every one except 4d, which needs the gateway-routed MCP path
this cluster's RHCL can't do) ran for real against alice/bob/charlie and
passed** — correct multi-hop tool chaining, correct sequential-dependency
resolution, and critically, the actual per-user tenant-isolation boundary
fired for real on cross-book access attempts (`MCP error -32602: client_id
no encontrado para el llamante autenticado`), not just a model
self-refusal — the core thing this demo exists to prove still holds on
0.1.2. Two distinct 0.1.2 regressions had to be found and fixed along the
way — a filesystem-permission bug (image-side fix) and a sandbox-creation
race condition (fixed at the root with `sandbox create --policy`, not
worked around) — see "Blocking finding", "Resolution", and "Second
blocking finding" below. Step 6 (Codex) still needs the separate
`inference.local` removal redesign before it can be tested the same way;
Scene 7 (audit trail) and Part II (red-team) are untested and blocked on
stale `claude-audit`/`codex-audit`/`claude-garak`/`codex-garak` images that
predate the base-image fix. Branch: `0.1.2-upgrade`. **Do not bump
`OPENSHELL_CHART_VERSION` or declare the demos upgraded** until those
remaining pieces are resolved too.

This covers `demos/base`, `demos/keycloak-oidc`, and `util/parrot`. 0.1.0 is
where essentially all the breaking changes land; 0.1.1 and 0.1.2 are patch
releases on top (WebSocket tunnel made opt-in, snap-gateway mTLS requirement,
a Homebrew-install config-overwrite fix, a supervisor CONNECT-header byte
count fix, and a loopback-access docs clarification — none of which affect
this repo).

Sources: NVIDIA's `/upgrade/0-1-0` guide, GitHub release notes for
v0.1.0/v0.1.1/v0.1.2, and — where docs were silent or inconsistent — direct
inspection of the published artifacts themselves (all read-only, no cluster
contact): `helm show values oci://ghcr.io/nvidia/openshell/helm-chart
--version 0.1.2`, `helm template` against our actual values files, the
0.1.2 CLI binary's own `--help` output (extracted from the Fedora rpm
release asset), the vendored `.proto`/Rust source at the `openshell-sdk`
git tag, and `podman run` against the new default sandbox image.

## Blocking finding — custom sandbox images fail to provision (live, confirmed root cause)

**Every custom sandbox image used by this demo (`CLAUDE_IMAGE`, `CODEX_IMAGE`,
and by extension the audit/garak variants built on top of them) fails to
provision under 0.1.2**, even though they were fine on 0.0.106. Confirmed
live on `keycloak-oidc-demo` namespace (OpenShift SCC UID range
`1000910000/10000`):

- `openshell sandbox create --from quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36 ...`
  creates the Sandbox resource, but the pod sits at `Init:Error` on the
  `workspace-init` container with `Error: × Permission denied (os error 13)`,
  and the gateway logs `rolled back stale fail-closed sandbox-runtime
  bootstrap`.
- Root cause, traced into the actual 0.1.2 gateway source
  (`crates/openshell-driver-kubernetes/src/driver.rs`,
  `resolve_sandbox_identity_in_namespace`): the Kubernetes compute driver is
  **new** in 0.1.2 in reading the namespace's `openshift.io/sa.scc.uid-range`
  / `openshift.io/sa.scc.supplemental-groups` annotations and forcing
  **both** `runAsUser` *and* `runAsGroup` on every sandbox pod (including the
  `workspace-init` init container that seeds the PVC from the image) to
  values derived from those annotations — in this case UID **and** GID
  `1000910000`.
- The images themselves do everything OpenShift's own "support arbitrary
  user IDs" convention asks for: `/sandbox` is `drwxrwx--- uid=1001(sandbox)
  gid=0(root)` (confirmed via `podman inspect`/`stat` on both
  `claude-sandbox:0.3.36` and the `codex` image — identical layout). That
  convention relies on the **group** staying `0` regardless of which
  arbitrary UID a pod ends up with. 0.1.2's driver instead sets the group to
  the *same* namespace-derived value as the UID (`1000910000`, not `0`),
  which isn't a member of the image's `root`-owned directory — hence
  `Permission denied` reading `/sandbox` to seed the workspace.
- This is not fixable from this repo's side: `sandbox_gid` can be overridden
  via a `gateway.toml` driver override, but the driver's own validation
  (`openshell_policy::MIN_SANDBOX_UID..=MAX_SANDBOX_UID`, i.e. `[1,
  u32::MAX-1]`) rejects `0` outright, so there's no way to configure the
  group OpenShift's own convention actually needs. Confirmed the **stock**
  default image (`nvcr.io/nvidia/base/ubuntu:24.04`, no pre-existing
  `/sandbox` content to seed) provisions fine under the same namespace — the
  bug only surfaces for images that ship real content under `/sandbox`.
- **This looks like an upstream OpenShell bug**, not a demo misconfiguration:
  the new OpenShift-SCC-awareness feature breaks the exact convention
  OpenShift itself documents for arbitrary-UID compatibility. Worth filing
  against `NVIDIA/OpenShell` before spending more effort here.
- **Not an image problem, confirmed**: also tested the newer
  `quay.io/aipcc/agentic-ci/claude-sandbox:0.4.0` and
  `quay.io/aipcc/agentic-ci/codex-sandbox:0.4.0` tags (both public, no
  registry login needed to pull). Identical `/sandbox` layout (`0770`,
  `uid=1001`, `gid=0`) and identical live failure
  (`workspace-init` → `Init:Error` → `Permission denied (os error 13)`) —
  rules out "stale image" as the cause and confirms the bug is in the
  0.1.2 gateway/driver, not in any particular image build. `CODEX_IMAGE`'s
  correct 0.4.0 path also changed repos:
  `quay.io/aipcc/base-images/agentic/codex:0.4.0` doesn't exist (404); the
  new tag lives at `quay.io/aipcc/agentic-ci/codex-sandbox:0.4.0` instead.
- **Confirmed via NVIDIA's own docs that this is intentional, not
  accidental**: `/kubernetes/openshift.md`, checked at both `/latest/` and
  the version-pinned `/v0.1.2/` path, states plainly: "The driver reads the
  namespace's `openshift.io/sa.scc.uid-range` annotation and uses the
  resulting UID/GID for the sandbox, agent, trusted init containers, and
  supervisor." Same value for UID and GID is the documented design, not a
  slip — it just isn't reconciled with the separate, equally standard
  OpenShift "arbitrary UID + group 0" image convention these images follow,
  and nothing in the docs warns custom-image authors about the conflict.

## Workaround confirmed live — rebuild the image with world-readable content

Since the injected UID *and* GID are both unpredictable at image-build time,
group-based tricks (GID 0, etc.) can't fix this from the image side — the
only thing that works is making the seeded content **world-readable**.
Tested end to end against this cluster:

```dockerfile
FROM quay.io/aipcc/agentic-ci/claude-sandbox:0.4.0
USER root
RUN chmod -R a+rX /sandbox
USER sandbox
```

Built via an OpenShift binary build straight into the cluster's internal
registry (`oc new-build --binary --strategy=docker` +
`oc start-build --from-dir=... --follow`), no external registry
credentials needed:

```bash
oc new-build --binary --name=claude-sandbox-fix --strategy=docker
oc start-build claude-sandbox-fix --from-dir=<dir-with-Dockerfile> --follow
# pushes to image-registry.openshift-image-registry.svc:5000/<ns>/claude-sandbox-fix:latest
```

Result: `openshell sandbox create --from image-registry.openshift-image-registry.svc:5000/keycloak-oidc-demo/claude-sandbox-fix:latest`
reached `Ready`/`DependenciesReady` with no `Init:Error`, and after
attaching providers and applying the policy, `claude --version` inside the
sandbox returned `2.1.263 (Claude Code)`. Seeded files end up correctly
owned by the runtime identity (`1000910000:1000910000` in this namespace)
post-extraction — `a+rX` only needs to survive the one-time read during
seeding, not persist as the live permissions.

**This is a real, scoped fix**: add one `chmod -R a+rX /sandbox` (or
whatever the workspace mount path is) layer, after all build steps, to any
custom sandbox image meant to run on OpenShell 0.1.2 + OpenShift. Doesn't
require an upstream fix — still worth filing upstream so other users don't
have to rediscover this, and so the docs gap gets closed, but it's no
longer a hard blocker for this demo.
## Resolution — rebuild our own claude-sandbox/codex-sandbox base images

Rather than waiting on an upstream fix, or relying on upstream republishing
`quay.io/aipcc/...` with the chmod fix, this repo now builds and owns its
own patched base images, the same pattern already used for
`claude-audit`/`claude-garak`/`codex-audit`/`codex-garak` (all of which
already layer Containerfiles on top of the upstream images):

- `demos/keycloak-oidc/images/claude-sandbox/Containerfile` — `FROM
  quay.io/aipcc/agentic-ci/claude-sandbox:0.4.0` + the `chmod -R a+rX
  /sandbox` fix. Publishes to `quay.io/atarazana/claude-sandbox:0.4.0`.
- `demos/keycloak-oidc/images/codex-sandbox/Containerfile` — same fix on
  `quay.io/aipcc/agentic-ci/codex-sandbox:0.4.0`, publishing to
  `quay.io/atarazana/codex-sandbox:0.4.0`. Both the codex tag and fix
  confirmed live the same way as claude's (`codex --version` →
  `codex-cli 0.153.4` inside a sandbox built from a throwaway in-cluster
  copy of this image).
- `claude-audit`, `claude-garak`, `codex-audit`, `codex-garak`'s
  Containerfiles now `FROM` our new images instead of the raw upstream
  ones (and picked up the 0.3.36→0.4.0 / old-repo-path→agentic-ci bumps in
  the same edit) — one fix, inherited everywhere, instead of four places
  to patch separately.
- `demos/keycloak-oidc/.env`'s `CLAUDE_IMAGE`/`CODEX_IMAGE` now point at
  `quay.io/atarazana/{claude,codex}-sandbox:0.4.0`.

**Published.** Both images are built and pushed —
`quay.io/atarazana/claude-sandbox:0.4.0`/`:latest` and
`quay.io/atarazana/codex-sandbox:0.4.0`/`:latest` — confirmed live via a
fresh `podman pull` after removing the local cache. One gotcha hit along
the way: quay.io defaults new repos to **private**, unlike this repo's
other `atarazana` images (confirmed via the quay.io API,
`"is_public": false` initially) — the cluster has no imagePullSecret for
that registry at all, relying on everything being public, so the first
real pull attempt failed with `ErrImagePull: unauthorized`. Fixed by
flipping both repos to public in the quay.io UI (no API/management token
available to do it from this session). `.env.example` still deliberately
hasn't been updated, same reasoning as the `OPENSHELL_CHART_VERSION`
cutover.

## Second blocking finding — sandbox create vs. the gateway's fail-closed rollback (a real race condition, fixed at the root cause)

Separate from the filesystem-permission bug above: even with a correctly
publicly-pullable, permission-fixed image, `scripts/15-provision-claude-sandbox.sh`
run as-is still failed. Root cause, confirmed live — this is a genuine race
condition between two independent, uncoordinated timers:

- **Clock 1 (gateway, server-side):** a sandbox created with `--provider`
  flags but no policy has **no valid policy at all** — a fresh sandbox
  does not get a usable built-in policy bundle the way earlier OpenShell
  versions did (`openshell policy get <sandbox> --base` returns "no active
  policy configured", not the documented built-in bundle). Its
  `configuration_admission` reports `state: "rejected"`, `error:
  "Effective configuration could not be activated; replace the policy or
  repair attached providers"` — reproduced even with **zero** providers
  attached, so this isn't a providers.v2/credential issue. After some
  variable amount of time in that rejected state (observed anywhere from
  ~30 seconds to 5 minutes across otherwise-identical attempts — not a
  fixed, documented timeout), the gateway's own "fail-closed" safeguard
  decides the sandbox is unrecoverable and deletes it outright (`rolled
  back stale fail-closed sandbox-runtime bootstrap` in the gateway logs).
- **Clock 2 (CLI, client-side):** `openshell sandbox create` itself blocks,
  polling for a Ready state that can never arrive without a policy — for
  up to its own ~5-minute timeout. The *only* thing that stops Clock 1 is
  calling `policy set` — but the original script's `sandbox create` call
  ran in the foreground and didn't return control to the script (so
  `policy set` could run) until it gave up.
- Whichever clock reaches zero first determines the outcome. The actual
  server-side `CreateSandbox` gRPC call completes in **under a second**
  regardless (confirmed in gateway logs: `CreateSandbox request completed
  successfully`) — the delay on both sides is independent client/server
  polling logic, not the actual sandbox creation.

**Not a known, already-filed upstream issue, but closely related to two
that are** (searched `NVIDIA/OpenShell` issues/PRs):
- [#3145](https://github.com/NVIDIA/OpenShell/issues/3145) (accepted,
  implemented via PR #3259) is the feature that *introduced* the
  "no active policy → rejected, stable error state" behavior — its own
  acceptance criteria say a policy-less sandbox should sit in a stable,
  externally-visible error state "repairable... without first activating
  the invalid workload configuration," i.e. exactly the state a
  `policy set` is supposed to be able to fix at leisure.
- [#3758](https://github.com/NVIDIA/OpenShell/issues/3758) (closed,
  self-withdrawn by the reporter) hits the identical reaper mechanism and
  log signature (`could not verify sandbox-runtime workload generation`
  → `rolled back stale fail-closed sandbox-runtime bootstrap`), traced to
  a hardcoded `SANDBOX_RUNTIME_BOOTSTRAP_GRACE = 5m` constant in
  `crates/openshell-driver-kubernetes/src/driver.rs` — not configurable via
  gateway.toml, Helm values, or any CLI flag — with no retry/backoff or
  consecutive-failure threshold before reaping. That issue's trigger was
  apiserver/etcd flakiness, not a missing policy, but it's the same reaper
  killing a sandbox that hasn't reached "ready" inside a fixed window,
  for any reason. Nobody appears to have reported the specific interaction
  between #3145's new "stable, repairable error state" and this
  unmodified, much older reaper — worth filing upstream.
- Checked for a CLI escape hatch first: `sandbox create --help` has no
  `--wait`/`--no-wait`/`--async`/`--timeout` flag, and `--detach` only
  skips interactive attach, not the readiness poll.

**The actual fix is not "win the race faster" — it's to avoid the race
entirely.** `sandbox create` has its own `--policy <file>` flag (easy to
miss: it's listed, undocumented-as-relevant-here, right next to `--from`
and `--provider` in `--help`). Rendering the policy *before* calling
`sandbox create` and passing it via `--policy` (alongside `--upload` for
the MCP config / config.toml, in the same call) means the sandbox is never
in the rejected state to begin with — there's no race to lose. Confirmed
live, repeatedly: `sandbox create --provider ... --provider ... --policy
... --upload ... --detach`, run as an ordinary foreground call with no
backgrounding/sleep tricks, returns normally in ordinary image-pull time
(~35s) and reaches `Ready` immediately, with a working `claude --version`.
An earlier, since-discarded version of this fix (background the `sandbox
create` call, `sleep 5`, then race `policy set` in immediately) also
worked, reliably, across repeated tests — but it was a workaround that won
the race, not a fix that removed it; `--policy`-at-creation is strictly
better and is what's actually committed.

Applied to both `scripts/15-provision-claude-sandbox.sh` (verified end to
end, reaches `Ready` + `claude --version` works) and
`scripts/14-provision-codex-sandbox.sh` (same mechanical fix applied for
consistency, but that script still can't be run end to end to re-confirm
it there — see the `openshell inference`/`inference.local` removal item
below, a separate, pre-existing blocker in the same script). The README's
"Provision the Claude Code harness" walkthrough was updated to match.

## Scenes 1–6 — live-verified on 0.1.2 (2026-10-02)

With all three bankers provisioned (same script, each run clean on the
first try: alice, bob, charlie all reached `Ready` with a working
`claude --version` and correct token substitution), each banker's own
`gateway add` login was redone fresh against this cluster (the
`oc-alice`/`oc-bob`/`oc-charlie` XDG identity dirs on disk predated it by
~3 weeks, from a different cluster), then every scene in the README was
run for real, via `openshell sandbox exec ... claude ...`, exactly as
written:

- **Scenes 1–3** (Bob: meeting prep, biggest-client resolution, dip
  diagnosis) — all passed. Correct multi-hop tool chaining in every case:
  `get_upcoming_meetings` before anything else in Scene 1,
  `get_top_client_by_aum` before `get_performance` in Scene 2,
  `get_positions` before a *scoped* `get_relevant_news` in Scene 3 (not a
  generic news dump). Scene 1 also handled a genuine mid-turn tool failure
  (an oversized payload) by reporting it honestly instead of fabricating —
  arguably better than the README's own example output.
- **Scene 4a–4c** (Bob overreaching, socially engineering, then asking for
  fabricated data) — all passed, including the one that matters most for
  this demo's whole premise: the direct "call the tool anyway" prompt
  produced a real, server-side denial — `MCP error -32602: client_id no
  encontrado para el llamante autenticado` — proving `assert_owns_client`
  tenant isolation actually fires on 0.1.2, not just that the model
  declines to ask. 4c correctly refused to fabricate a substitute client
  portfolio after the real one was denied, offering a clearly-labeled
  hypothetical instead. (4d skipped — gateway-routed MCP, needs the
  `MCPGatewayExtension` CRD this cluster's RHCL doesn't have.)
- **Scenes 5a–5b** (Charlie: compliance escalation reasoning, product
  suitability) — passed. Client name ("Fundación Iris") resolved to
  `cli-005` via `mcp-portfolio` before any `mcp-kyc-compliance` call in
  both scenes; 5a cited two real regulatory documents by name rather than
  a flat yes/no; 5b's suitability verdicts matched the README's
  documented expected results exactly (prod-002 unsuitable, prod-001
  potentially suitable pending KYC/PEP sign-off).
- **Scene 6** (Alice: boundary from the other side + the
  `compatibility-user` second permission) — passed, 3 parts. Part 1: Alice
  refused at the reasoning layer before ever calling a tool for Bob's
  client (a different, equally valid outcome from 4a's forced-call case,
  exactly as the README anticipates). Part 2 hit a transient `calc_tax`
  connection error on first try — retried once, per the README's own
  documented transient-MCP-flakiness note, and passed cleanly on retry
  (€5,712, matching the README's example almost exactly). Part 3
  (self-contained tax question, no client data involved) passed on the
  first try with the exact numbers from the README's own example
  (€17,340 for a 90,000 income in Lysmark).

No code or script changes were needed for any of this — Scenes 1–6 run
on 0.1.2 exactly as documented, once the two provisioning-layer bugs
above are fixed.

## Gateway-routed MCP (`--gw`) — working for the first time (2026-10-02)

Every earlier run of this demo, on every cluster, used **direct** per-server
access; Scene 4d was always skipped. Running the `--gw` path end to end
surfaced three separate problems, all now fixed, and 4d passes.

**1. The user credential was never allowed to reach the gateway host.**
`providers/user-refresh-profile.yaml` lists only the five in-cluster
`mcp-*.svc.cluster.local:8000` endpoints. In `--gw` mode the sandbox talks
to `mcp.<apps-domain>:443` instead, which isn't in that allowlist, so the
proxy refuses to inject `USER_ACCESS_TOKEN` and the gateway answers
`{"error":"credential_endpoint_mismatch"}`. `git log -S` confirms that host
has never been in the file in any commit — the gateway credential path had
simply never worked. Added it as a `<mcp-gateway-host>` placeholder
endpoint.

**2. Nothing created the OpenShift `Route` for `gateway.publicHost`.** The
`mcp-gateway` chart deliberately forces the gateway Service to ClusterIP, so
without a Route the public host reaches the ingress router with no backend
and returns a bare `HTTP/1.0 503`. sandbox268 had one hand-created, which is
why it looked fine there. Added `mcp-gateway/templates/route.yaml` (edge/
Redirect, `targetPort: mcp` by name — both listeners share a port, so only
the name disambiguates).

**3. MCP Gateway Operator assumes upstream Istio's Service naming.** The
operator launches the broker with
`--mcp-gateway-private-host=<gateway-name>-istio.<ns>.svc.cluster.local:8080`,
but OpenShift's own Gateway API controller names the Service
`<gateway-name>-<gatewayclass-name>`. With any `className` other than
`istio` that host doesn't resolve, and the broker can't dial itself back
through the Gateway. **This is the deceptive one:** `tools/list` still
succeeds, because the broker serves it from its own aggregated cache without
leaving the pod — an agent sees all 16 tools and still cannot call a single
one, failing with `failed to create session for mcp server: transport
error: dial tcp: lookup mcp-gateway-istio... no such host`. Worked around
with an `ExternalName` alias
(`mcp-gateway/templates/broker-private-host-alias.yaml`), guarded by
`ne .Values.gateway.className "istio"` so it disappears once the operator
derives the name correctly. Worth filing upstream.

**Verified live, gateway-routed:** bob → `Clara Fontán (cli-001), $38,750`;
alice → `Elena Duarte (cli-004), $33,000` (correctly different books through
one shared broker endpoint); and **Scene 4d / 4a's cross-tenant denial fired
for real** — `MCP error -32602: client_id no encontrado para el llamante
autenticado` when bob reached for alice's `cli-004`, a server-side ownership
check, not a model self-refusal.

### Operational gotcha: `provider profile update` invalidates a baked MCP config

The `Authorization: Bearer openshell:resolve:env:s<random>_USER_ACCESS_TOKEN`
placeholder baked into `/sandbox/.claude/mcp-servers.json` is **per provider
attachment**. Running `openshell provider profile update` rotates the
attachment and regenerates that random component, so the already-uploaded
file silently points at a binding that no longer exists. The symptom is a
misleading `403 A credential placeholder in the request body cannot be
forwarded ... body credential rewriting is disabled` — note its own
"restore provider access" hint. Do **not** reach for the
`request_body_credential_rewrite` endpoint option to fix it: that resolves
the placeholder *into* the request body, i.e. ships the user's real Keycloak
token to the LLM provider, defeating the isolation the demo exists to prove.
The fix is to delete and re-provision the sandbox so the upload carries the
current placeholder (`--upload` does not re-apply to an existing sandbox).
To check: compare the value in the file against `printenv USER_ACCESS_TOKEN`
inside the sandbox — they must be identical.

## Scene 7 (audit trail) — working, after four stacked silent failures

Scene 7 produced no audit signal at all, and every layer of the failure was
invisible: the sandbox was `Ready`, the agent answered prompts correctly,
Prometheus was up and scraping the collector, and the hook exited 0. Four
separate bugs had to be cleared.

**1. The audited sandbox ran the wrong image.**
`16-provision-audited-sandbox.sh` exports `CLAUDE_IMAGE`/`CODEX_IMAGE` as
the `*-audit` image before invoking 14/15, but those scripts then run
`set -a; source .env`, which reassigns both back to the plain image.
`SANDBOX_PREFIX` isn't in `.env`, so it survives — producing a correctly
named `aud-` sandbox with no `session-auditor` binary in it. Both scripts
now preserve a caller-supplied image override across the `.env` load.

**2. The auditor's egress was denied by binary identity.** `session-auditor`
doesn't open sockets itself — it shells out to `curl` for both the
classification call and the OTLP POST. The profiles listed only
`/usr/local/bin/session-auditor`, and 0.1.2's supervisor enforces
`require_binary_identity:true`, so every request was denied. `curl -f`
fails, the hook swallows the error, exit code stays 0. Added
`/usr/bin/curl` to both auditor profiles.

**3. Short Service names don't resolve in a sandbox.** The binary posted to
`http://audit-collector:4318`. The supervisor's policy DNS doesn't resolve
bare Service names (`Could not resolve host`), while the FQDN it *does*
resolve was refused (`Permission denied`) because the profile authorized
the literal string `audit-collector`. Both the baked-in endpoint and the
profile entry are now the FQDN; `scripts/16` substitutes
`<openshell-namespace>` into the profile the same way `onboard` does for
the user profile. Note this binds the published audit images to a
namespace — deliberate, because the endpoint is compile-time by design
(an agent must not be able to redirect or silence its own audit signal),
so it cannot become an env var.

**4. Same tag, stale image.** The Makefile tagged images by base-image
version alone, so rebuilding with changed auditor content produced an
identical tag; sandbox pods run `imagePullPolicy: IfNotPresent`, so nodes
kept serving the cached binary and the endpoint fix appeared to do nothing
(verified by grepping the running binary — still the old short name).
Images are now tagged `<base>-<crate-version>`, e.g.
`claude-audit:0.4.0-0.1.5`.

**Verified live, gateway-routed:** benign turn →
`agent_session_started` + `agent_turn_heartbeat`; forced overreach →
`session_compliance_risk_score{sandbox="aud-claude-bob",workspace="bob"} 2`,
surfaced by the dashboard API as
`risk_level: "blocked_attempt"` — the amber verdict the README predicts,
and the ceiling for this scene since the server-side check denies the call
outright.

**Open:** the dashboard reports `mcp_servers: []`, so the graph has no
user → sandbox → server edges yet. Partly the known trace-consumption
follow-up, and partly inherent to `--gw`: every call goes to one broker
endpoint, so per-server edges collapse unless the edges are derived from
tool-name prefixes instead of destination hosts.

## Codex (Annex A) — unblocked, working with direct MCP

`openshell inference` was removed in 0.1.2, taking `inference.local` (the
privacy router) with it, so scripts/14 could not run at all. Codex now
talks to the LLM endpoint directly, exactly as the Claude Code harness
always has, with the credential injected by the `byo-codex` provider
profile instead of by the router.

**The endpoint has to be a Responses-API one.** codex-cli rejects
`wire_api = "chat"` outright since 0.146 — a hard config-load error, not
a deprecation — and sends MCP tools as `"type": "namespace"` tools that
only the Responses API carries. Hence new `CODEX_*` variables rather than
reusing `OPENAI_*`: the model, and therefore the API shape, differs from
what the chat-completions consumers (mcp-market-news' generator,
session-auditor) use.

**MaaS can serve it, contrary to a first reading.** `/v1/responses`
against the stock `deepseek-flash` 500s, and the `ExternalModel` CRD
documents only `openai-chat` and `messages` — which together look
conclusive but are not. `apiFormat` is a free-form string, and
`openai-responses` works; the name came from the gateway's own rejection
of a wrong guess:

```
ext_proc_error ... unsupported format combination: openai-responses → responses
```

i.e. the ext_proc already recognises the incoming shape as
`openai-responses`. Publishing a model with that format and
`path: /responses` makes Codex work through MaaS like everything else —
see `demos/keycloak-oidc/maas/`. Worth confirming with the MaaS folks,
since it is undocumented.

Two traps on the client side: MaaS serves these under `/v1`, so
`CODEX_BASE_URL` must end in `/v1` or codex-cli posts to `/responses` and
gets `BadRequest - unsupported API endpoint`; and a model is invisible
until it has both a `MaaSModelRef` and a `MaaSSubscription` entry, failing
with `403 subscription ... does not include model` until then.

Also needed: `--dangerously-bypass-approvals-and-sandbox` on `codex exec`.
Without it every MCP call dies with "MCP tool call requires approval, but
approval policy is never". It is the Codex analogue of the Claude recipe's
`--permission-mode bypassPermissions`, and its own help text says it is
"intended solely for running in environments that are externally
sandboxed" — which is precisely what OpenShell is here.

**Verified live, audited + direct MCP** (`16-provision-audited-sandbox.sh
bob codex ...`): `mcp: mcp-portfolio/get_top_client_by_aum (completed)` →
"Clara Fontán (cli-001), $38,750", the same answer the Claude harness
gives, with session-auditor's Stop hook firing.

### Codex + `--gw`: a broker protocol-version bug, worked around

Gateway-routed Codex failed at first: tool *discovery* succeeded, then
every invocation died with the broker's own error (`component=router`):

```
failed to create session for mcp server: failed to create client:
transport error: server returned 4xx for initialize POST, likely a legacy SSE server
```

That message is a red herring — nothing is a legacy SSE server. Tracing
it down: Authorino reported `authorized: true`, so not auth; the Gateway
access log showed the broker (`mcp-router`) getting **400** from the
backend; and the backend's own log gave the real reason:

```
rejecting initialize: MCP-Protocol-Version header does not match
params.protocolVersion   header="2025-06-18" body="2025-11-25"
```

**The broker relays the downstream client's negotiated MCP revision in
the JSON body but keeps its own, older value in the `MCP-Protocol-Version`
header.** The backend's rmcp server rejects the disagreement with 400.
Claude Code negotiates 2025-06-18 — the same revision as the broker — so
header and body agree and it never trips; codex-cli negotiates 2025-11-25
and every tool call fails, while discovery (which needs no upstream
session) keeps working and makes it look like a partial outage.

Worked around in this repo rather than waiting on the broker:
`mcp-servers`' HTTPRoutes now strip `MCP-Protocol-Version` on the way to
each backend (`mcpGateway.stripProtocolVersionHeader`, default on). rmcp
only enforces the match when the header is present and otherwise honours
the body's version — which is the one the client actually asked for.
Verified directly against a backend: initialize succeeds for every
revision from 2024-11-05 to 2026-02-06 when no header is sent.

**Verified live after the fix**, audited + gateway-routed:
`mcp: gateway/mcp_portfolio_get_top_client_by_aum (completed)` →
"Clara Fontán (cli-001), 38,750.0", with session-auditor's Stop hook
firing and `aud-codex-bob` appearing on the audit dashboard with
per-server edges.

Still cosmetic: codex gets `DELETE /mcp → 403` on session teardown, after
the answer is delivered — most likely the authz AuthPolicy's CEL
predicate finding no JSON-RPC body on a DELETE. Worth filing upstream
alongside the header bug and the `<gateway>-istio` naming bug.

### Codex "Model metadata not found" — leave it alone

Every codex turn prints:

```
warning: Model metadata for `deepseek-flash-responses` not found.
Defaulting to fallback metadata; this can degrade performance and cause issues.
```

codex resolves the model against a catalog bundled with its binary, which
only knows OpenAI's own slugs, so any MaaS/BYO name misses. The warning
is not purely cosmetic — the fallback supplies the context window and
auto-compact threshold.

**Both fixes were tried on `aud-codex-bob` and both were rejected.**

`model_context_window` / `model_max_output_tokens` do not silence it; the
warning still prints every turn (upstream
[openai/codex#21070](https://github.com/openai/codex/issues/21070)).

`model_catalog_json` does silence it — and breaks tool calling outright.
codex refuses a catalog entry carrying neither `base_instructions` nor
`model_messages.instructions_template`, so the entry has to be cloned
from a real one (`gpt-6-astra` is the first with instructions), which
drags GPT-specific instructions and capability flags along with it. The
BYO model cannot honour them. Same sandbox, same prompt, only the
`model_catalog_json` line differing:

| | warning | MCP tool calls | DSML leaked |
|---|---|---|---|
| catalog ON | 0 | **0** | 10 |
| catalog OFF | 1 | **2** | 0 |

With the catalog on, the model stopped invoking tools and instead emitted
raw `<|DSML|>` tool syntax as literal text in its reply — the same
leakage class seen when MCP never connects. Trading every tool call for a
quieter log is a bad deal, so the warning stays.

Revisit only if codex gains a way to supply metadata without
instructions, or the gateway serves a catalog. There is a comment in
`scripts/14-provision-codex-sandbox.sh` at the `config.toml` heredoc
recording this, so the experiment is not repeated.

## Already fixed on this branch (verified locally, no cluster needed)

- **`util/parrot`**: ran `make bump-openshell-sdk OPENSHELL_SDK_TAG=v0.1.2`,
  then fixed the 3 resulting compile errors against the real 0.1.2 SDK:
  - `client.rs`: `list_workspaces(...).await` → `.collect_all().await`
    (`list_workspaces` now returns a lazy `Pager<WorkspaceRef>`, not a
    future — pagination is one of the documented 0.1.0 breaking changes).
  - `exec.rs`: `ExecSandboxRequest.sandbox_id` (removed) → `sandbox: String`
    (canonical name, not an internal id — matches the documented "public
    sandbox RPCs use name instead of id" change). The existing
    `get_sandbox()` existence/membership check is kept for its fail-fast
    behavior, just no longer used for an id.
  - `exec.rs`: `ExecSandboxRequest.timeout_seconds` (removed) →
    `execution_timeout: Option<prost_types::Duration>` (the documented
    scalar-time → protobuf `Duration`/`Timestamp` change). Added
    `prost-types = "0.14"` to `util/parrot/Cargo.toml` and
    `parrot-core/Cargo.toml` to construct it.
  - Also had to add `workspace_scope: Some(proto::workspace_selector(...))`
    to the same request — a new required-in-practice field.
  - `cargo check -p parrot-core -p parrot-tui -p parrot-gpui` all pass;
    `cargo test -p parrot-core` passes 39/39.
- **`demos/keycloak-oidc/policies/templates/policy.yaml`**: removed
  `tls: terminate` from the `api.anthropic.com` endpoint — that enum value
  is removed in 0.1.0 (omitting `tls:` now gets the same auto-inspection
  behavior; `tls: skip` is *not* an equivalent substitute, it disables
  inspection).
- **`demos/keycloak-oidc/docs/policy-anatomy.md`**: updated the matching
  example snippet and the `tls` field's table description to match.

## Live-verified on 0.1.2 (sandbox341, fresh install, Part I steps 1–4)

- **Step 1 (Keycloak)**: RHBK operator + Keycloak CR + realm import —
  unaffected by the OpenShell version, passed as documented.
- **Step 2a (Helm install)**: `helm upgrade --install openshell
  oci://ghcr.io/nvidia/openshell/helm-chart --version 0.1.2 -f
  helm/values-certmanager.yaml ...` succeeded first try with the existing
  values file, no changes needed — confirms the earlier `helm template`
  prediction. Gateway pod came up, Route created, cert-manager-issued certs
  worked.
- **Step 2b/2c (`gateway add` + OIDC login)**: worked unchanged, including
  the Playwright-driven headless login.
- **Step 2d (Providers v2) — found a real change**: `openshell settings set
  --global --key providers_v2_enabled --value true` now fails with `unknown
  setting key 'providers_v2_enabled'. Allowed keys: ocsf_json_enabled,
  ocsf_schema_version, agent_policy_proposals_enabled,
  proposal_approval_mode`. Providers v2 behavior (the whole `provider
  profile import` → `provider create` → `provider refresh
  configure/rotate` flow) works regardless — **this step is gone/obsolete
  in 0.1.2, Providers v2 is just always-on.** Skip it when updating the
  guide; no replacement command needed.
- **Step 3 (`onboard` tool, all three bankers)**: ran headlessly via
  Playwright exactly as documented, including workspace create/member-add.
  `openshell provider refresh status` showed `STATUS: refreshed` for
  alice/bob/charlie — fully working on 0.1.2, no script changes needed.
- **Step 4a (mcp-gateway / RHCL)** — **resolved, environment gap, not a
  0.1.2 regression, and not actually an RHCL version problem either.**
  Originally fell back to direct access
  (`MCP_GATEWAY_ENABLED=false`/`MCP_GATEWAY_AUTH_ENABLED=false`) because
  `helm install ./mcp-gateway` failed with `no matches for kind
  "MCPGatewayExtension"` — `rhcl-operator.v1.4.3` alone has no
  `mcp.kuadrant.io` CRDs. Root-caused later: gateway-routed MCP needs a
  **second, separate OLM operator** (`mcp-gateway`, "MCP Gateway Operator
  (Tech Preview)", channel `preview`, versioned `0.7.x` — unrelated to
  RHCL's own `1.4.x`), which this demo's docs never mentioned installing
  at all. Installed it live
  (`oc get packagemanifest mcp-gateway -n openshift-marketplace` confirmed
  it's in the same `Red Hat Operators` catalog), which produced the
  missing CRDs immediately. Found one more gap finishing the install: the
  `MCPGatewayExtension` then sat at `Ready: False, Reason:
  ReferenceGrantRequired` (its target `Gateway` lives in a different
  namespace, `openshift-ingress` vs. `mcp-gateway-system`) — the
  `mcp-gateway` chart never templated the `ReferenceGrant` that needs;
  fixed by adding `templates/referencegrant.yaml` (chart bumped to 0.2.0).
  `helm install ./mcp-gateway` now succeeds clean on a cluster that only
  ever had the documented prerequisites, `mcp-gateway-ext` reaches
  `Ready: True`, and step 4a's own verification
  (`GatewayClass`/`Gateway`/`MCPGatewayExtension`) all pass. Documented
  both gaps in the README (new "Installing the MCP Gateway Operator"
  section) and in the chart's own Chart.yaml/values.yaml/README. **Scene
  4d and the `--gw` sandboxes (`claude-alice-gw`/`claude-bob-gw`) are now
  testable on this cluster** — not yet actually re-run, see "Remaining
  work" below.
  **RHOAI `mcplifecycleoperator` investigation — related but distinct
  feature, not a substitute:** investigated whether RHOAI's
  `DataScienceCluster.spec.components.mcplifecycleoperator` is a
  declarative/RHOAI-native alternative to the manual OLM Subscription
  above (prompted by finding it set to `Managed` on sandbox268 alongside a
  manually-named-identically `mcp-gateway` Subscription). First pass on
  sandbox341: deleted the manual Subscription/CSV, flipped
  `mcplifecycleoperator` to `Managed`, and watched RHOAI deploy a
  *different* operator — `mcp-lifecycle-operator-controller-manager` in
  `redhat-ods-applications`, reconciling `MCPServer` CRs (API group
  `mcp.x-k8s.io`) — not RHCL/Kuadrant's
  `MCPGatewayExtension`/`MCPServerRegistration` (`mcp.kuadrant.io`) this
  chart needs. Reverted (`mcplifecycleoperator` back to `Removed`,
  reapplied the manual Subscription from `/tmp/mcp-gateway-operator.yaml`,
  confirmed `mcp-gateway-ext` back to `Ready: True` within ~20s) while
  investigating further.
  Follow-up (same session): this is actually RHOAI's documented **AI Hub →
  MCP Catalog** feature (GA'd Technology Preview in 3.4/3.5 — see [Red
  Hat's MCP catalog blog
  post](https://www.redhat.com/en/blog/mcp-catalog-here-discover-deploy-and-connect-red-hat-openshift-ai)
  and the [3.5 Technology Preview release
  notes](https://docs.redhat.com/en/documentation/red_hat_openshift_ai_self-managed/3.5/html/release_notes/technology-preview-features_relnotes)),
  not a coincidence. Dashboard visibility needs a second flag beyond the
  DSC component: `OdhDashboardConfig.spec.dashboardConfig.mcpCatalog:
  true` (our initial check of `dashboardConfig`'s key list predated
  setting this — the field doesn't appear until set). Re-enabled
  `mcplifecycleoperator: Managed` plus `mcpCatalog`/`autorag: true` (with
  `genAiStudio` already `true`) on sandbox341, restarted `rhods-dashboard`
  to pick up the config, confirmed no collision with the existing
  `mcp-gateway` Subscription/CSV/`mcp-gateway-ext` (still `Ready: True`
  throughout). Left both features enabled this time, since the user wants
  AI Hub's MCP Catalog available. Red Hat's own docs confirm the
  distinction is intentional, not a doc gap: the Kuadrant-based **MCP
  gateway Operator** (manual OLM Subscription, what this chart needs) is
  explicitly "an optional external prerequisite... not managed by the
  OpenShift AI operator lifecycle," while the **MCP Lifecycle
  Operator**/AI Hub catalog is RHOAI-native and manages a different
  concept (deploying MCP *server* workloads from a catalog, not
  gateway/auth routing for already-deployed servers). Documented the full,
  accurate distinction in `mcp-gateway/README.md`'s Gotcha section.
- **Step 4b (MCP servers, direct access)**: `./scripts/06-deploy-mcp-servers.sh`
  worked unchanged. Hit the exact fresh-cluster embeddings cold-start
  CrashLoopBackOff the README already documents
  (`mcp-market-news`/`mcp-kyc-compliance` failing until
  `jina-embeddings-v3-cpu-predictor` finishes loading) — self-healed within
  a few minutes with no intervention, confirming that troubleshooting note
  is still accurate on 0.1.2.
- **`sandbox create --upload ... -- true` — found a real breaking change,
  not in NVIDIA's docs**: 0.1.2 flat-out rejects `--upload` combined with a
  trailing `[COMMAND]` (`error: the argument '--upload <UPLOAD>' cannot be
  used with '[COMMAND]...'`), regardless of whether `--from` is also given.
  Fixed by replacing `-- true` with `--detach` everywhere this pattern
  appears — confirmed live that `--detach` produces the same "create +
  upload, don't attach" behavior `-- true` used to. Fixed in
  `scripts/15-provision-claude-sandbox.sh`, `scripts/14-provision-codex-sandbox.sh`,
  and the matching README snippet (step 5, "Provision the Claude Code
  harness"). This pattern is otherwise unused in `demos/base`.
- **Default sandbox image lacking `curl`**: same finding as `demos/base`'s
  hello-world (see below), but it also silently affected
  `scripts/15-provision-claude-sandbox.sh`, whose own comment ("Claude Code
  is pre-installed in the chart's default sandbox image") is now false.
  Added `CLAUDE_IMAGE=quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36` to
  `.env` (this repo's own known-good image, already used elsewhere in
  `docs/evalhub-redteam.md`) as a required override — except that image hits
  the blocking finding above, so this alone isn't sufficient yet.

## Confirmed non-issues (don't touch these)

Checked against the real 0.1.0 breaking-changes list and ruled out:

- **`NetworkBinary.harness` / binary shape** — every policy and provider
  profile in this repo already uses the post-0.1.0 object/scalar binary
  shape. Nothing to change (the word "harness" only appears in prose
  comments).
- **Provider alias renames (`claude`→`claude-code`, `gh`→`github`)** — this
  repo never relies on gateway-shipped built-in profiles; all provider
  profiles here are custom IDs (`byo-claude`, `byo-codex`, etc.), so there's
  no collision.
- **L7 rule-edit flags (`--rule-name`/`--any-binary`)** — verified against
  the real 0.1.2 CLI (`openshell policy update --help`): those flags are
  only required for `--add-allow`/`--add-deny` (L7 method/path rule
  appends). This repo only ever uses `--add-endpoint --binary`, whose
  syntax is **unchanged** in 0.1.2. Grepped the whole repo for
  `add-allow`/`add-deny` — zero hits. (An earlier automated sweep flagged
  ~10 `policy update` call sites here as broken; that was a false positive
  from reading the breaking-changes prose without checking which code path
  each call site actually uses.)
- **`sandbox create --from` must be a pre-built image** — already true
  everywhere in this repo except `demos/base`'s hello-world step (see
  below, that one never passed `--from` in the first place).
- **Trailing sandbox command auto-provider-attach removal** — every
  `sandbox create` here already passes `--provider` inline or does an
  explicit separate `provider attach` call.
- **`WorkspaceSelector` required / no default-to-`default`** — this repo
  already passes `--workspace <id>` explicitly everywhere, and the 0.1.2
  CLI's own docs confirm the CLI (as opposed to the raw API) still defaults
  `--workspace` to `default` when omitted, so there's no behavior gap here
  either way.
- **Helm chart gateway/workspace split** — rendered both
  `demos/keycloak-oidc/helm/values.yaml` and `values-certmanager.yaml`
  against the real 0.1.2 chart with `helm template` (local, no cluster).
  Both render cleanly, unchanged flag/value names
  (`server.auth.allowUnauthenticatedUsers`, `server.oidc.*`,
  `certManager.*`, `openshiftRoute.*`, `podSecurityContext.fsGroup`,
  `securityContext.runAsUser` all still exist as-is in 0.1.2's
  `values.yaml`). `workspaceResources.enabled` defaults to `true`, which is
  what this demo needs (single namespace, not split gateway/workspace
  releases) — confirmed the rendered output includes the sandbox
  ServiceAccount/Role/RoleBinding/NetworkPolicy in the same release. No
  values-file changes needed. The "schema-v2 `gateway.toml`" migration in
  the breaking-changes doc is for hand-authored `gateway.toml` files
  (standalone/snap/binary installs) — confirmed via NVIDIA's gateway
  configuration doc that Helm generates a compliant file itself; flat
  `values.yaml` keys need no restructuring.
- **Old fixed-name RBAC (`openshell-gateway-node-reader`)** — rendered
  manifest shows namespace-qualified names already
  (`openshell-node-reader-<namespace>`). Only matters for an **in-place**
  `helm upgrade` of an existing 0.0.106 release (delete the old
  ClusterRole/ClusterRoleBinding after confirming the new ones exist) — not
  a repo-file change. This demo's install flow is a fresh install, so
  flag this as an operator run-step, not a code change, if/when testing
  against an existing 0.0.106 gateway.
- **`gateway.toml` / credential-driver / `platform_config.host_users` /
  `compute_driver` settings** — no gateway.toml is hand-authored anywhere
  in this repo (the Helm chart generates it); none of these settings are
  referenced. Not applicable.
- **MCP `MCP-Protocol-Version` header** — no MCP client/server code here
  sets protocol-version headers explicitly; this is an SDK-dependency
  concern for `mcp/mcp-*`, not something this repo's own code does wrong.
  Worth a dependency-version check once on a cluster where the MCP flow can
  actually be exercised, not a static-analysis fix.

## Still required — concrete, low-risk (no live testing needed to decide, but verify after)

- **Version pins**: `OPENSHELL_CHART_VERSION=0.0.106` → `0.1.2` in
  `demos/base/.env.example:4` and `demos/keycloak-oidc/.env.example:4`,
  plus every doc that echoes `0.0.106` as *the version this guide targets*
  (not the historical "as of 0.0.106" claims below):
  `README.md:3` (root badge), `demos/base/README.md:18,36,53-55,280,284-285,642`,
  `demos/keycloak-oidc/README.md:379,399,416-418,610,614-615,632`,
  `util/onboarding-web/Containerfile:23,25`. **Deliberately not done yet**
  — this is the actual cutover marker and should land after the rest of
  this list is live-verified, not before.
- **`demos/base` hello-world default image** (`demos/base/scripts/04-hello-world-sandbox.sh`,
  `demos/base/README.md` step 1): confirmed locally via `podman run
  nvcr.io/nvidia/base/ubuntu:24.04` (the new chart default
  `sandbox.image.*`, replacing the removed `server.sandboxImage` key that
  used to default to `ghcr.io/nvidia/openshell-community/sandboxes/base:latest`)
  that **curl is not installed** in the new default image. The
  `sandbox create --name hello-world -- bash` call doesn't pass `--from`,
  so it gets this minimal image. The demo's whole point at that step is
  "the policy blocks the request, not a missing binary" — under 0.1.2 it'll
  instead fail with `curl: command not found` before policy ever gets
  involved, breaking the narrative. Needs a decision (pin `--from` to a
  curl-capable image, or set `sandbox.image.repository/tag` at Helm install
  time) plus a live run to confirm the resulting error message still
  teaches the intended lesson. Also update the troubleshooting line at
  `demos/base/README.md:646` if the fix changes which binary path readers
  are told to check.

## Still required — needs a live cluster to design/verify

- **Codex "BYO LLM via privacy router" path (Annex A)** — the biggest
  unknown. `openshell inference` and the `inference.local` endpoint are
  **removed** in 0.1.0 (confirmed: 0.1.2's CLI `--help` has no `inference`
  command at all). This repo's Codex provisioning depends on both:
  - `demos/keycloak-oidc/scripts/14-provision-codex-sandbox.sh:112`
    (`openshell inference set ...`), `:149`
    (`base_url = "https://inference.local/v1"`), `:207`
    (`--set llmHost=inference.local`)
  - `demos/keycloak-oidc/providers/byo-codex-profile.yaml:16` and
    `deepseek-codex-profile.yaml:21` (`host: inference.local`)
  - `README.md:3241,3260,3264,3292,3350` (architecture prose + diagram)
  - `exam/questions.md:62-64,84`, `exam/answers.md:95-97,169,171`
  NVIDIA's replacement pattern (per their "Migrate from Managed Inference
  Routes" doc) is: export/edit/re-import the provider profile with the
  *real* native endpoint (e.g. a BYO LLM host) instead of the
  gateway-internal `inference.local` alias, attach it per-sandbox, and have
  the workload call that native endpoint directly — the same
  provider-profile-based pattern already used for Claude Code's
  `byo-claude-profile.yaml`. This is a real redesign of Annex A's Codex
  flow, not a find/replace, and needs to be built and run against a live
  gateway to confirm the resulting policy/credential wiring still proves
  the same "centralized BYO LLM, no raw API key in the sandbox" point the
  demo exists to make.
- **Live OIDC + sandbox + policy end-to-end retest** — even where the
  values/CLI surface is confirmed compatible above, actually running
  step 2a's Helm install, Keycloak OIDC login, `gateway add`, sandbox
  provisioning, and the MCP authorization scripts against OpenShift is the
  only way to catch anything the docs/local tooling couldn't surface
  (timing, SCC/OpenShift-specific behavior, cert issuance, the exact
  wording of any new CLI error messages scripts grep for).
- **Historical "as of 0.0.106 / confirmed on 0.0.106" claims** —
  deliberately left untouched: `evalhub-redteam.md:951,1079`,
  `evalhub-redteam-orig.md:373,813`, `manual-onboarding.md:56`,
  `self-service-onboarding.md:4`, `exam/questions.md:56,58`,
  `exam/answers.md:87`, `README.md:1726`, `docs/sandbox-service-patterns.md:6`,
  `docs/headless-browser-automation.md:214,249`. These describe when a
  behavior was introduced or last verified, not what version the demo
  currently targets — re-verify each live on 0.1.2 before touching the
  wording (most are probably still accurate, since they describe
  not-yet-removed features, but "probably" isn't good enough to assert in
  a guide).

## Remaining work, in order

1. ~~Publish `quay.io/atarazana/claude-sandbox:0.4.0` and
   `quay.io/atarazana/codex-sandbox:0.4.0`~~ — **done.** Both built, pushed,
   and flipped to public; `scripts/15-provision-claude-sandbox.sh` verified
   end to end against the real published images, unmodified, reaching
   `Ready` with a working `claude --version`. Still worth filing both
   findings upstream against `NVIDIA/OpenShell` (the GID-0 image-convention
   gap and the sandbox-create-vs-fail-closed-rollback race) even with
   workarounds in hand, since both will bite the next OpenShift user.
2. ~~Walk scenes 1–6 for alice/bob/charlie~~ — **done**, see "Scenes 1–6"
   above. Only Scene 4d (gateway-routed MCP) and Scene 7 (audit trail)
   remain untested from step 5.
   ~~Get gateway-routed MCP infrastructure working on this cluster~~ —
   **done**, see "Step 4a" above (`mcp-gateway` operator + `ReferenceGrant`
   fix). What's left for Scene 4d specifically: deploy `mcp-servers` with
   `MCP_GATEWAY_ENABLED=true`/`MCP_GATEWAY_AUTH_ENABLED=true`, provision
   `claude-alice-gw`/`claude-bob-gw` (`15-provision-claude-sandbox.sh --gw`),
   and actually re-run Scene 4d — none of that has happened yet, only the
   cluster-wide prerequisite is now in place.
3. **Scene 7 ("Watching the audit trail live")** — not started. Blocked on
   stale images: `claude-audit`/`codex-audit` (published at
   `quay.io/atarazana`, tagged `0.3.36`/`0.0.1-1786355012`) predate the
   Containerfile re-point to our fixed `claude-sandbox:0.4.0`/
   `codex-sandbox:0.4.0` base images, so they still carry the GID bug.
   Needs `util/session-auditor`'s `make image-claude image-codex
   push-claude push-codex` re-run (which also needs the session-auditor
   musl binary rebuilt first), plus `audit-collector`/`audit-dashboard`/
   `audit-tempo` deployed on this cluster (none of the three are installed
   yet) before it can be tested at all.
4. **Part II — red-team eval (EvalHub + Garak)** — entirely untested on
   0.1.2. Same staleness problem: `claude-garak`/`codex-garak` are
   published only as `:latest` (no version-pinned tag), predating the
   base-image fix, not rebuilt either.
5. Redesign and provision the Codex BYO-LLM flow (step 6 / Annex A) against
   the new provider-profile pattern, replacing the removed `openshell
   inference`/`inference.local` mechanism (`scripts/14-provision-codex-sandbox.sh`
   line ~112, `openshell inference set`, has no 0.1.2 equivalent at all) —
   still untested beyond confirming `codex-sandbox`'s own GID fix and the
   create/policy/upload timing fix, both applied to that script already.
   The biggest remaining piece of work in this list.
6. Re-run `demos/base`'s hello-world step to settle the default-image/curl
   fix there.
7. ~~Separately, get this cluster's RHCL to a version that ships
   `MCPGatewayExtension`~~ — **done**, see item 2 above (it was never an
   RHCL version problem — a second operator).
8. Only then: bump `OPENSHELL_CHART_VERSION` everywhere, update the
   historical-claim docs that turned out to need it, and merge
   `0.1.2-upgrade` back into `main`.
