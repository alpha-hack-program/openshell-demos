# OpenShell 0.0.106 → 0.1.2 upgrade — prep notes

Status: **research/prep complete, demo cutover pending a live cluster.**
Branch: `0.1.2-upgrade`. Do not bump `OPENSHELL_CHART_VERSION` or declare the
demos upgraded until the remaining items below are verified against a real
OpenShift cluster — several of them (Helm install behavior, OIDC flow,
sandbox policy enforcement) can't be confirmed from docs or local tooling
alone.

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

## Suggested order once cluster access is available

1. Fresh Helm install of the keycloak-oidc stack at 0.1.2 (not an in-place
   upgrade — simpler, and this repo's docs already assume fresh installs).
2. Walk Part I of the README literally (per house rule: run the README's
   commands, not `scripts/NN-*.sh`) through OIDC login and `gateway add`.
3. Provision Claude Code sandbox + policy (`policies/` chart) — confirms the
   `tls: terminate` removal didn't regress anything.
4. Redesign and provision the Codex BYO-LLM flow against the new
   provider-profile pattern.
5. Re-run `demos/base`'s hello-world step to settle the default-image fix.
6. Only then: bump `OPENSHELL_CHART_VERSION` everywhere, update the
   historical-claim docs that turned out to need it, and merge
   `0.1.2-upgrade` back into `main`.
