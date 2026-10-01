# OpenShell 0.0.106 → 0.1.2 upgrade — prep notes

Status: **live-tested against a fresh OpenShift cluster (sandbox341,
OCP 4.22.15) on 2026-10-01.** Part I steps 1–4 (Keycloak, gateway install,
OIDC login, Providers v2 onboarding, MCP servers) pass on 0.1.2 with the
fixes below. Step 5 (Claude Code sandbox provisioning) is now **fully
working end to end** — `quay.io/atarazana/claude-sandbox:0.4.0` and
`quay.io/atarazana/codex-sandbox:0.4.0` are built, pushed, and public, and
`scripts/15-provision-claude-sandbox.sh` (run unmodified, for real) reaches
`PHASE: Ready` with a working `claude --version`. Two distinct 0.1.2
regressions had to be found and worked around along the way — a
filesystem-permission bug (image-side fix) and a sandbox-creation timing
race (script-side fix) — see "Blocking finding", "Resolution", and "Second
blocking finding" below. Step 6 (Codex) still needs the separate
`inference.local` removal redesign before it can be tested the same way.
Branch: `0.1.2-upgrade`. **Do not bump `OPENSHELL_CHART_VERSION` or declare
the demos upgraded** until step 6 and the remaining scenes are re-verified
too.

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

## Second blocking finding — sandbox create blocks and gets rolled back (script-side, fixed)

Separate from the filesystem-permission bug above: even with a correctly
publicly-pullable, permission-fixed image, `scripts/15-provision-claude-sandbox.sh`
run as-is still failed. Root cause, confirmed live:

- A sandbox created with `--provider` flags attached has **no valid policy
  yet** — a fresh sandbox does not get a usable built-in policy bundle the
  way earlier OpenShell versions did (`openshell policy get <sandbox>
  --base` returns "no active policy configured", not the documented
  built-in bundle). Its `configuration_admission` reports `state:
  "rejected"`, `error: "Effective configuration could not be activated;
  replace the policy or repair attached providers"` — reproduced even with
  **zero** providers attached, so this isn't a providers.v2/credential
  issue, it's that a brand-new sandbox simply has no admitted policy until
  one is explicitly set.
- `openshell sandbox create` itself then sits in its own client-side
  readiness-polling loop waiting for that non-existent Ready state —
  anywhere from ~30 seconds to the full 5-minute client timeout observed
  across different attempts — while the **gateway's own fail-closed
  safeguard** independently rolls back and deletes the still-rejected
  sandbox after its own timeout (`rolled back stale fail-closed
  sandbox-runtime bootstrap` in the gateway logs). Whichever fires first,
  the script's later `openshell policy set` call — the thing that would
  actually fix the rejected configuration — never gets a chance to run
  before the sandbox is gone.
- The actual server-side `CreateSandbox` gRPC call completes in **under a
  second** regardless (confirmed in gateway logs: `CreateSandbox request
  completed successfully`) — the multi-minute delay is entirely the CLI's
  own client-side wait, which `--detach` does not skip.

**Fix, confirmed live repeatedly**: background the `sandbox create` call
(`&`), `sleep 5`, then immediately call `policy set --wait` — this
reliably wins the race every time tested, reaching `Ready` with a working
agent binary. `--upload` had to come out of the `sandbox create` call
entirely in the process: bundling it into a backgrounded/abandoned create
was found to silently lose the uploaded file (confirmed: the 10-second
cutoff in one test killed the client before "Uploading files..." printed).
Config is now uploaded via a separate `openshell sandbox upload <name>
<local> <dest>` call (positional, not the `local:remote` syntax `sandbox
create --upload` uses) once the sandbox has an active policy, then token
substitution proceeds as before. Applied to both
`scripts/15-provision-claude-sandbox.sh` (verified end to end, unmodified,
reaches `Ready` + `claude --version` works) and
`scripts/14-provision-codex-sandbox.sh` (same mechanical fix applied for
consistency, but that script still can't be run end to end — see the
`openshell inference`/`inference.local` removal item below, a separate,
pre-existing blocker in the same script).

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
- **Step 4a (mcp-gateway / RHCL)** — **environment gap, not a 0.1.2
  regression**: this cluster's RHCL operator (`rhcl-operator.v1.4.3`, the
  only channel OperatorHub offers) has no `mcp.kuadrant.io/v1alpha1`
  `MCPGatewayExtension` CRD at all (`oc get crd | grep mcp` → nothing), so
  `helm install ./mcp-gateway` fails with `no matches for kind
  "MCPGatewayExtension"`. Not related to the OpenShell version — this is an
  RHCL/Kuadrant version gap on this specific cluster. Fell back to the
  guide's documented direct-access alternative
  (`MCP_GATEWAY_ENABLED=false`/`MCP_GATEWAY_AUTH_ENABLED=false`) and
  continued; didn't investigate further since it's orthogonal to this task.
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
2. Walk the rest of step 5's scenes (1–7) for alice/bob/charlie against the
   now-working Claude Code sandboxes — only banker `alice`'s sandbox has
   been provisioned and smoke-tested (`id`, `claude --version`, MCP config
   token substitution) so far; bob and charlie, and the actual scene
   scripts/assertions, are still unverified on 0.1.2.
3. Redesign and provision the Codex BYO-LLM flow (step 6 / Annex A) against
   the new provider-profile pattern, replacing the removed `openshell
   inference`/`inference.local` mechanism (`scripts/14-provision-codex-sandbox.sh`
   line ~112, `openshell inference set`, has no 0.1.2 equivalent at all) —
   still untested beyond confirming `codex-sandbox`'s own GID fix and the
   create/policy/upload timing fix, both applied to that script already.
4. Re-run `demos/base`'s hello-world step to settle the default-image/curl
   fix there.
5. Separately, get this cluster's RHCL operator (or a different cluster) to
   a version that actually ships `MCPGatewayExtension`, to test the
   gateway-routed MCP path (`--gw`) and Scene 4d, which direct-access skips.
6. Only then: bump `OPENSHELL_CHART_VERSION` everywhere, update the
   historical-claim docs that turned out to need it, and merge
   `0.1.2-upgrade` back into `main`.
