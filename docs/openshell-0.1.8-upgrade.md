# OpenShell 0.1.4 → 0.1.8 upgrade — prep notes

Status: **live-tested as a genuine in-place upgrade against an actively-used
OpenShift cluster (sandbox341, OCP 4.22.15) on 2026-11-14.** Unlike the
0.1.2/0.1.4 upgrades (both done against a freshly torn-down namespace), this
one ran `helm upgrade --install` directly against the existing `openshell`
Helm release (revision 1, 24 days old, `helm-chart-0.1.4`) in the
`keycloak-oidc-demo` namespace, with six live sandboxes and five other Helm
releases (`mcp-servers`, `audit`, `audit-dashboard`, `audit-tempo`,
`onboarding-web`) already running underneath it. Branch: `v0.1.8-upgrade`,
branched from `main`. **Cut over**: `OPENSHELL_CHART_VERSION` is now `0.1.8`
in `demos/keycloak-oidc/.env.example` and every doc passage that echoes it as
the guide's target version, following live verification of the main recipe
(steps 1–5 including `scripts/08-verify-isolation.sh`). Per the user's own
decision partway through this pass, **all six pre-existing sandboxes were
deliberately deleted and reprovisioned from scratch** rather than preserved
— see "The live upgrade incident" below for what that uncovered. **Not
merged into `main`** — a colleague may be actively working from `main`, so
this branch is left standing on its own, promoted to "confirmed working"
status without disturbing anyone else's checkout.

`demos/base` is explicitly **out of scope** and stays pinned at whatever
version it was last bumped to independently.

Sources: GitHub release notes for v0.1.5/v0.1.6/v0.1.7/v0.1.8 (fetched via
the unauthenticated GitHub API, same as the 0.1.2/0.1.4 passes), `helm show
values`/`helm template` diffed against the real 0.1.4→0.1.8 chart (including
pulling the chart source to read the actual Secret-generation template, not
just trusting the rendered diff), direct inspection of every provider
profile and policy file in this demo, and live testing against the cluster
— including one real incident, described in full below rather than glossed
over.

## What changed 0.1.4 → 0.1.8, and what it means for this demo

### Helm chart — backward-compatible, only cosmetic/hardening diffs

Rendered both `demos/keycloak-oidc/helm/values.yaml` and
`values-certmanager.yaml` against the real 0.1.8 chart (`helm template` with
`--set agentSandbox.preflight.enabled=false` for **offline rendering only**
— the same known non-issue the 0.1.4 doc documented: `helm template` has no
live API discovery, so the Agent Sandbox CRD preflight check fails
regardless of version; a real `helm upgrade` against this cluster, which
does have the CRDs installed, is unaffected). Diffing the two renders turned
up nothing that changes this demo's actual behavior:

- Every key this demo already sets (`podSecurityContext.fsGroup`,
  `securityContext.runAsUser`, `server.auth.allowUnauthenticatedUsers`,
  `server.oidc.*`, `server.providerTokenGrants.spiffe`,
  `openshiftRoute.*`, `certManager.*`) is unchanged.
- **Gateway container and the `certgen` (PKI init job) init container both
  now default to `seccompProfile: RuntimeDefault`**, and `certgen` also
  gains `runAsNonRoot: true` and drops all capabilities. This is a
  security-hardening default, not a behavior change this demo relies on —
  confirmed live, the gateway pod and certgen job both ran cleanly under the
  tightened defaults.
- **New ephemeral `gateway-state` `emptyDir` volume** mounted at
  `/var/lib/openshell-state`, with `XDG_STATE_HOME` set to match. Per the
  chart's own new comment, this is where "the gateway stages normalized,
  content-addressed supervisor trust artifacts" — infrastructure for a
  feature this demo doesn't otherwise touch. Ephemeral, no PVC, no action
  needed.
- **New `supervisor-hpa.yaml`/`supervisor-networkpolicy.yaml`/
  `supervisor-pdb.yaml`/`supervisor-service.yaml`/
  `supervisor-token-secret.yaml` templates** all render empty — they're
  gated behind the v0.1.6 experimental shared-supervisor-pool feature
  (`supervisor_topology = "gateway"`), which neither of this demo's values
  files set. Confirmed non-issue.
- `server.extraEnv` (0.1.6), `priorityClassName`/`topologySpreadConstraints`
  (0.1.8), and `metrics.tls.*` (0.1.8) are all new, all opt-in, all default
  to off/empty. Non-issues.

**One thing worth flagging explicitly because it looked alarming at
first**: diffing two separate offline `helm template` runs showed the
`key-encryption-key` Secret's value differing between the 0.1.4 and 0.1.8
renders. This is **not** a real behavior change — pulled the chart source
(`templates/credential-storage-key-encryption-key-secret.yaml`) and
confirmed it uses `lookup "v1" "Secret" .Release.Namespace $secretName`: if
the Secret already exists in the live cluster, its stored value is reused
verbatim; a new random value is only generated when none exists. Offline
`helm template` has no live cluster access, so `lookup` always misses and a
fresh random value appears on every render — pure rendering noise, present
in both 0.1.4 and 0.1.8 and not something the version bump introduced. The
Secret also carries `helm.sh/resource-policy: keep`, so it survives even a
full release uninstall. Confirmed live: the real `helm upgrade` did **not**
rotate this value, and every banker's already-issued provider credentials
kept working unchanged after the upgrade.

### Breaking changes checked individually, all confirmed non-issues for this demo

- **Static provider credential placement (v0.1.8, PR #4886)** — "Static
  provider credentials without placement metadata now resolve only in the
  `Authorization` header or through explicitly enabled body/WebSocket
  rewrite policies... update existing imported profiles as needed, since
  editing reference YAML alone does not update the gateway catalog."
  Checked every file under `demos/keycloak-oidc/providers/*.yaml`: every
  credential already declares an explicit `auth_style` (`bearer` or
  `header`) with `header_name` set — no query-param or path-slot
  placements anywhere. `token-exchange-profile.yaml` has no `auth_style` on
  its one credential, but that file is explicitly marked `DRAFT ONLY,
  schema unconfirmed`/`[VERIFY]` and isn't live-imported anywhere — flagged
  for whoever finishes that draft, not a blocker here.
  **However**, see "The live upgrade incident" below — this breaking
  change's own warning ("editing reference YAML alone does not update the
  gateway catalog") turned out to matter in a different, more subtle way
  than a stale-YAML content mismatch.
- **OCSF sandbox attribution field move (v0.1.8, PR #5048)** and **OCSF
  schema version (v0.1.7, native 1.8.0 only, PR #5036)** — this demo's audit
  pipeline (`session-auditor` → `audit-collector` OTel collector →
  `audit-tempo`/Prometheus → `audit-dashboard`) is entirely self-contained:
  `session-auditor`'s own Stop-hook pushes build their own span/metric
  attributes directly (`detect_sandbox_identity()` in
  `util/session-auditor/src/main.rs`), never derived from the gateway's
  native OCSF audit-log stream. Grepped both `session-auditor` and
  `audit-dashboard` source for `container.uid`/`container.name`/
  `unmapped.*`/OCSF field names — zero hits. The gateway's native OCSF
  emission is never wired into this demo's collector at all. Non-issue.
- **Gateway auth requirement (v0.1.6, PR #4821)** — gateways now refuse to
  start unless OIDC, mTLS, or gateway JWTs are configured (or
  `allow_unauthenticated_users = true` explicitly set). This demo's entire
  premise is Keycloak OIDC — both values files already set
  `server.auth.allowUnauthenticatedUsers: false` and fully configure
  `server.oidc.issuer/audience/rolesClaim/adminRole/userRole`. Non-issue,
  confirmed live (the gateway's actual startup failure this round was a
  self-inflicted config mistake, not this gate — see below).
- **Sandbox control-connection direction change (v0.1.6, PR #4656)** — "New
  sandboxes... open the control connection from the sandbox to its
  supervisor... Upgrade the supervisor and sandbox runtime binaries
  together, because older binaries reject the new sandboxes." This is a
  different connection than the CLI/SDK↔gateway public API that
  `util/parrot`'s pinned SDK talks to (see below) — it's the
  Kubernetes-driver-internal supervisor↔gateway control channel, which
  `docs/openshell-networking.md` doesn't document at all. Rather than
  assume the changelog's own reassurance ("running sandboxes keep their
  existing connection direction") held, this was **verified live**: all six
  pre-existing supervisor pods successfully reconnected and resumed
  reporting sandbox state to the upgraded 0.1.8 gateway immediately after
  the gateway pod came back up — confirmed via gateway logs showing
  `reconcile.sandboxes`/`GetSandboxConfig`/`OCSF AUTH:LOGON ALLOWED` for
  every pre-existing `sandbox.id`, and `provider credential refresh
  completed` cycling normally for all three bankers. The supervisor pods
  themselves briefly failed their readiness probe (`dial tcp ...:5501:
  connection refused`) during the gateway restart window but recovered on
  their own without intervention. (All six were deliberately deleted and
  reprovisioned shortly after anyway, per the user's own choice to verify
  from scratch — see below — so this is a point-in-time confirmation, not
  a claim about indefinite long-term coexistence.)
- **WebSocket 403 (v0.1.8, PR #5336) and symlink-policy (v0.1.8, PR
  #5278)** — grepped `demos/keycloak-oidc/policies/templates/policy.yaml`:
  every `network_policies` entry uses `protocol: rest`, zero
  `protocol: websocket`/`websocket_credential_rewrite` entries; every
  `filesystem_policy` path is a fixed real path, none under a symlinked
  writable-workspace mount. Non-issue.

### Everything else from the four changelogs — one line each

- PostgreSQL-unavailable fast-fail (#5244) and the deleted-sandbox gateway
  memory leak fix (#5428) / stream-buffer pre-allocation fix (#5408) — pure
  reliability improvements, directly relevant to *why* this upgrade is
  worth doing on a long-lived gateway process (the pre-upgrade gateway pod
  was 24 days old).
- Sandbox delete no longer waits out the pod's 30s termination grace period
  (#5279) — confirmed live, deleting all nine sandboxes during this pass
  was noticeably fast.
- Provider-attachment credential-inspection timing fix (#5310: checks
  against the sandbox's *latest saved* policy instead of its *creation*
  policy) — this demo's `--policy`-at-creation workaround (documented in
  `docs/openshell-0.1.2-upgrade.md`'s "Second blocking finding") passes
  policy at creation time already, so the distinction this fix addresses
  doesn't change this demo's call pattern either way.
- Sandbox suspend/resume (#4765), metrics TLS listener (#3877), supervisor
  route advertisement (#4940), `supervisor_runtime_class_name` (#5349),
  Helm `priorityClassName`/`topologySpreadConstraints` (#2832) — all new,
  all opt-in, all unused by this demo.
- `openshell status --gateway <name>` now exits non-zero for an
  unregistered gateway (#5379) — grepped `demos/keycloak-oidc/scripts/*.sh`
  for `status --gateway`; no hits, only a prose mention in the README
  echoing `--help` output. Non-issue.
- CLI OIDC client-credentials token renewal fix (#4389) — only applies to
  `OPENSHELL_OIDC_CLIENT_SECRET`-based client-credentials grants; this
  demo's flow is interactive browser OIDC login. Non-issue.

### `util/parrot` — SDK bump, one compile fix, released as parrot-v0.1.3

`make bump-openshell-sdk OPENSHELL_SDK_TAG=v0.1.8` (run with `gsed` instead
of the Makefile's own `sed -i -E` invocation — BSD `sed`'s `-E` dialect
rejects the escaped-brace pattern the Makefile uses; this is the same
macOS/BSD-vs-GNU gap the 0.1.4 doc already catalogued for this repo's shell
scripts, now also true of this Makefile target on macOS, not fixed in the
Makefile itself since a working `gsed` is already the documented
workaround elsewhere in this repo) pulled in the v0.1.8 SDK cleanly except
for one real compile error:

`OidcTokenBundle` (in `openshell-bootstrap`) gained a `client_credentials:
bool` field in v0.1.8 ("whether the token came from the `client_credentials`
grant, so it can be renewed by repeating that grant"). `parrot-core`'s
`StoredOidcRefresher::refresh()` constructs a new `OidcTokenBundle` after
rotating via refresh token and was missing this field. Fixed by carrying it
forward from the loaded bundle, the same way `issuer`/`client_id` already
are (`parrot-core/src/auth.rs`) — a refresh-token rotation never changes
whether the *original* token came from a client-credentials grant, so this
is the correct value, not a placeholder.

`cargo check -p parrot-core -p parrot-tui` and `cargo test -p parrot-core`
(39/39 tests) both pass clean against the new SDK. `parrot-gpui` fails to
build on this session's x86_64 macOS host:

```
src/bindings.h:1:9: fatal error: 'CoreMedia/CMFormatDescription.h' file not found
thread 'main' panicked ...: assertion `left == right` failed: "x86_64-apple-darwin" "x86_64-apple-darwin"
  left: 4
 right: 8
```

Confirmed via `git diff Cargo.lock` that `gpui`'s own pinned version is
**completely untouched** by this SDK bump — this is a pre-existing,
environment-specific gap (`gpui`'s bindgen/Clang CoreMedia binding
generation on Intel macOS), not a regression. `ci-parrot.yml`'s own job
matrix only exercises `macos-latest` (Apple Silicon) and `ubuntu-latest`,
confirming upstream CI has never covered this exact platform either.

**Released `parrot-v0.1.3`** (parrot's own crate version — unrelated number
to the OpenShell SDK tag it's pinned to) via `make release-patch`
(`cargo-release`), but not from this machine: `cargo-release`'s
`pre-release-hook` runs `make ci`, which runs workspace-wide
`cargo clippy --all-targets`, which pulls in the same blocked
`parrot-gpui` target. Ran it instead from a Fedora 44 toolbox on a separate
Linux build host (`carlos-kenshin`, already set up with `cargo`/`rustc`,
`libxcb-devel`/`libxkbcommon-devel`/`libxkbcommon-x11-devel`, and an
authenticated GitHub SSH key) — `make ci` passed clean there (clippy
included `parrot-gpui` successfully, since Linux doesn't need CoreMedia),
and `make release-patch` bumped the workspace version, committed, tagged
`parrot-v0.1.3`, and pushed. The `Release parrot` GitHub Actions workflow
then built and published both platforms' binaries
(`parrot-linux-x86_64`, `parrot-macos-aarch64`, plus the `-gpui` variants) —
confirmed via the GitHub API that the release is non-draft,
non-prerelease, with all four assets present. `demos/keycloak-oidc/
README.md`'s pinned install tag is updated from `parrot-v0.1.1` to
`parrot-v0.1.3`.

**Functionally verified against the live 0.1.8 gateway**: a direct
`./target/debug/parrot -s claude-bob -w bob -p "What is 2+2?..."` smoke
test, driven non-interactively via `expect` (see "Automating parrot's
interactive TUI" below for why that needed its own fix), reached
`completed`/`final result` with no errors once `ANTHROPIC_BASE_URL`/
`ANTHROPIC_MODEL` were correctly forwarded from the calling shell — parrot
talks to the upgraded gateway and drives a real Claude Code turn
end-to-end. There is no `parrot-macos-x86_64` release asset (matches the
CI matrix: only Linux x86_64 and macOS **aarch64** are built), so this
smoke test used a locally-built debug binary rather than the published
release asset on this particular machine — a pre-existing gap in asset
coverage, not something this upgrade changed.

### `util/onboarding-web` — also needed a lockstep CLI bump (not originally in scope, added because the same risk class applies)

Not called out in the original task framing (only `util/parrot`'s SDK pin
was), but `util/onboarding-web/Containerfile` hard-pins
`ARG OPENSHELL_VERSION` and downloads that exact CLI release's musl
tarball into the image — its own comment documents a **real, previously
observed live failure** from this exact mismatch class: an image built
against the 0.0.106 CLI talking to a 0.1.2 gateway failed every login with
`failed to decode Protobuf message: ListProvidersRequest.workspace_scope:
buffer underflow`. Bumped `ARG OPENSHELL_VERSION` to `0.1.8`, and bumped
`Cargo.toml`/the image tag from `0.1.1` to **`0.1.2`** (not a
Containerfile-only rebuild under the existing tag) — deliberately a new
tag rather than reusing `0.1.1`, because `onboarding-web/values.yaml` sets
`imagePullPolicy: IfNotPresent`, and reusing a tag risks a node serving its
already-cached stale image instead of pulling the rebuilt one.

Built successfully on `carlos-kenshin` (same Linux host used for the
parrot release) via `podman build`. **Push to `quay.io/atarazana/
onboarding-web:0.1.2` is blocked** — that host's `quay.io` login has
expired or was never configured for this session (`unauthorized: access to
the requested resource is not authorized`); fixing this needs either
interactive `podman login` on that host or a robot-account credential,
neither of which this session has. The built image, and the committed
Containerfile/Cargo.toml/values.yaml changes, are ready — pushing the
image and running `scripts/11-deploy-onboarding-web.sh` to redeploy is
tracked as remaining work below.

## The live upgrade incident

Unlike the 0.1.2/0.1.4 passes, this upgrade ran directly against a live,
in-use release. It did not go cleanly, and the mistake and recovery are
documented here in full rather than cleaned up after the fact.

**What went wrong:** the first `helm upgrade --install openshell ... -f
helm/values-certmanager.yaml` omitted the `--set` overrides that the
README's own documented install command requires
(`server.oidc.issuer`, `openshiftRoute.host`, and the `certManager.
serverDnsNames`/`serverIssuerRef` SAN overrides) —
`values-certmanager.yaml` ships with a literal `<keycloak-host>` placeholder
in the OIDC issuer URL specifically because it's meant to be filled in via
`--set` at install time, not edited in place. Running without those
overrides put the literal placeholder string into the live config. The
gateway pod immediately crash-looped:

```
Error:   × configuration error: OIDC initialization failed: OIDC issuer must be an
  │ absolute URL: invalid international domain name
```

**Why rollback wasn't available:** before this was caught, the gateway
container had already run far enough (past `sizing SQLite connection pool`)
to apply a forward database migration before hitting the OIDC
configuration error. Attempting to restore service by reverting to
`helm upgrade --install ... --version 0.1.4` (with the corrected `--set`
flags this time) **also failed**, with a different, more serious error:

```
Error:   × execution error: migration error: migration 9 was previously applied but
  │ is missing in the resolved migrations
```

The 0.1.4 gateway binary's own migration set doesn't know about whatever
migration 9 represents, because 0.1.8 introduced it. Once a gateway binary
applies a forward migration, there is no going back to an older binary
version against that same database — the only way forward was to fix the
*configuration* and move forward to 0.1.8, not to revert the *version*.

**Recovery:** re-ran the upgrade at `--version 0.1.8` with the full,
correct `--set` overrides (OIDC issuer, Route host, cert SANs) copied
verbatim from the README's own "Install command" section. The StatefulSet's
rolling update didn't automatically replace the crash-looping pod with the
corrected template on its own within a reasonable window (observed
`currentRevision` staying stale behind `updateRevision` while the old pod
kept crash-looping on the old config) — `oc delete pod openshell-0` forced
immediate recreation under the new, correct template each time this was
needed. The gateway came up clean on the second attempt at 0.1.8, and
**started reconciling all six pre-existing sandboxes successfully within
seconds** — a direct, live confirmation that the v0.1.6 supervisor
connection-direction change's own claim (existing sandboxes keep working
across a gateway bump) holds in practice, not just in the changelog's own
words.

**Total user-facing downtime**: roughly 6 minutes (14:51–14:57 CET).

**Lesson for anyone repeating this upgrade against a live release**: do not
run `helm upgrade --install` against `values-certmanager.yaml` with only
`-f` — always reconstruct the full `--set` override set from the README's
"Install command" section (OIDC issuer, Route host, cert SANs, and the
conditional `server.oidc.caConfigMapName` if the self-signed-ingress-cert
ConfigMap exists), the same way the README's own documented install
command does. There is no abbreviated safe form of this command for an
OIDC-based install.

## Deliberate decision: sandboxes recreated from scratch, not preserved

Given the live-upgrade incident above, and before running the Helm upgrade,
this session surfaced the sandbox-survival question explicitly rather than
assuming. The decision made: **all six pre-existing sandboxes
(`alice--aud-claude-alice`, `alice--aud-codex-alice`, `bob--claude-bob`,
`bob--aud-codex-bob`, `bob--aud-claude-bob`, `charlie--claude-charlie`) were
deliberately deleted** rather than preserved or carefully migrated, to get
a genuine from-scratch verification on 0.1.8 instead of a partial one.

**One consequence worth flagging for anyone picking this cluster back up**:
`bob--aud-claude-bob` was, at the time of deletion, the active sandbox for
a separate, unmerged branch's in-progress work
(`mlflow-tracing-claude-sandbox` — MLflow session tracing for Claude Code
sandboxes). Recreating sandboxes from this branch's tooling (based on
`main`, which has no knowledge of that other branch's provider/policy/
Containerfile additions) means the new `bob` sandboxes do **not** carry that
branch's MLflow wiring, even though the underlying `claude-sandbox:0.4.2`
image still has MLflow baked in (built and pushed independently of which
branch is checked out locally). Re-wiring MLflow onto the current,
0.1.8-upgraded sandboxes — and testing whether v0.1.8's
`[openshell.supervisor.network].additional_ca_bundle` (landed in v0.1.7,
see the mlflow branch's own root-cause notes) actually resolves the
original TLS-trust blocker — is a separate, explicit follow-up, not part
of this upgrade's own scope.

Nine fresh sandboxes were reprovisioned under 0.1.8: three plain
`claude-<user>` (gateway-routed, via `15-provision-claude-sandbox.sh --gw`)
for alice/bob/charlie, plus six audited (`aud-claude-<user>`/
`aud-codex-<user>`, via `17-provision-scene7-both-agents.sh --gw`) for all
three bankers. All nine reached `Ready` on the first attempt.

## Automating parrot's interactive TUI for headless verification

Worth documenting since it cost real time and produced one genuine,
reusable fix: `parrot`'s dashboard (ratatui-based) needs a real pty and
normally waits for a keypress at the end of each turn. Driving it
non-interactively via `expect` initially appeared to hang indefinitely —
turns out `expect`'s spawned pty defaults to a **0×0 window size** when
`expect` itself isn't attached to a controlling terminal (true in a
sandboxed/headless shell), and `ratatui`/`crossterm` spin forever trying to
redraw into a zero-area terminal instead of erroring out. Setting
`stty_init "rows 50 columns 200"` before `spawn` fixes this completely —
confirmed live, a `parrot -p "..."` one-shot prompt that previously timed
out after 900s with nothing but raw ANSI reset sequences in the log now
renders a real dashboard, completes the turn, and reaches the
`[q] quit` prompt reliably.

Separately: running `parrot` directly without first sourcing this demo's
`.env` or `scripts/as.sh` produces a believable-looking but misleading
failure — `ANTHROPIC_BASE_URL`/`ANTHROPIC_MODEL` are **not** baked into the
sandbox's persistent environment (only the credential itself,
`ANTHROPIC_AUTH_TOKEN`, is injected via the provider); they're meant to be
forwarded per-exec via `--env`, which `parrot` auto-detects from the
*calling shell's* environment. Without them, Claude Code falls back to its
hardcoded default (`api.anthropic.com`), and the MaaS/BYO credential is
*correctly* refused for that endpoint:

```
» Failed to authenticate. API Error: 403 Credential is not authorized for this request endpoint
```

This looks exactly like a credential/endpoint authorization regression
(and was chased as one for a while, including an unnecessary
`openshell provider profile update byo-claude` — harmless, but didn't fix
anything, since the live profile already had correct `auth_style`/
`header_name` metadata). A raw `curl` through the same sandbox, same
credential, same endpoint, same model returned `HTTP 200` immediately,
which is what actually isolated the real cause: always `source .env` (or
`scripts/as.sh <user>`) in the calling shell before invoking `parrot`
directly outside of the demo's own wrapper scripts.

**Full audited multi-scene verification** (all scenes, both agents, across
all three bankers, via `scripts/19-run-scene7-persona-demo.sh --aud`) was
attempted but not completed in this pass — two parallel attempts spent
significant time on the pty-sizing issue above plus a possible
concurrent-load stall when running multiple personas' `parrot` sessions
back to back against the same gateway/MaaS backend simultaneously. Given
the main recipe (steps 1–5 plus the 19/19 isolation check) and a direct
functional `parrot` smoke test were both independently confirmed working,
the decision made was to treat 0.1.8 as verified without chasing full
multi-scene TUI coverage to completion — see "Remaining work" below.

## Live-verified on 0.1.8 (sandbox341, in-place upgrade, 2026-11-14)

- **Live Helm upgrade** — `helm upgrade --install openshell
  oci://ghcr.io/nvidia/openshell/helm-chart --version 0.1.8 -f
  helm/values-certmanager.yaml` with the full `--set` override set,
  against the existing in-use release (see incident above for the
  first-attempt failure and recovery). `helm history` shows a clean final
  state: revision 4, `STATUS: deployed`, `helm-chart-0.1.8`.
- **Gateway health** — `openshell-0` pod `Running`, no further crashes.
  Scanned gateway logs post-recovery for anything beyond benign gRPC status
  codes and routine Kubernetes watch-stream renewal warnings (`resourceVersion
  ... too old: Expired` — normal, self-healing, not an upgrade regression) —
  no real errors found.
- **Local CLI** — rebuilt from source at tag `v0.1.8` via a direct
  `cargo install --path crates/openshell-cli` against this machine's
  existing Rust toolchain (far faster than Homebrew's own formula, which
  rebuilds LLVM/Rust from scratch on this Intel Mac since no x86_64 macOS
  bottle exists upstream for either Homebrew's formula or OpenShell's own
  GitHub release assets — only `aarch64-apple-darwin` is published).
  `openshell status` confirms `Version: 0.1.8`; the admin's pre-existing
  OIDC session (stored refresh token from before the upgrade) kept working
  unchanged against the upgraded gateway with no re-login needed.
- **Sandbox reprovisioning** — all nine fresh sandboxes
  (`claude-alice`/`claude-bob`/`claude-charlie`,
  `aud-claude-<user>`/`aud-codex-<user>` ×3) reached `Ready` on the first
  attempt, no GID/SCC workarounds needed beyond what 0.1.2/0.1.4 already
  established (the `quay.io/atarazana/{claude,codex}-sandbox` rebuilt base
  images are unaffected by this bump — confirmed by their successful use
  here).
- **`scripts/08-verify-isolation.sh`** — **19/19 checks passed**, identical
  result to the 0.1.4 doc's own run: all three bankers reach their four
  shared servers, only Alice reaches `mcp-compatibility`, and all four of
  Bob's cross-tenant probes against Alice's and Charlie's clients are
  denied with no data leaked.
- **`util/parrot`** — SDK bumped, one compile fix, 39/39 `parrot-core`
  tests pass, released as `parrot-v0.1.3`, and functionally verified end
  to end against the live 0.1.8 gateway via a real Claude Code turn.
- **`util/onboarding-web`** — CLI bump committed and image built; push to
  `quay.io` and redeploy are blocked on registry auth, tracked below.
- **Version cutover** — `OPENSHELL_CHART_VERSION=0.1.8` in
  `demos/keycloak-oidc/.env.example` and every README passage that echoes
  it as the guide's current target version (historical "as of"/"confirmed
  on 0.1.4" claims deliberately left untouched, same discipline as the
  0.1.4 doc).

## Remaining work

1. **Push `quay.io/atarazana/onboarding-web:0.1.2`** from `carlos-kenshin`
   (needs a fresh `podman login quay.io` there, or a robot-account
   credential — not available to this session) and then run
   `scripts/11-deploy-onboarding-web.sh` to redeploy the live
   `onboarding-web` Helm release and verify the admin login/onboarding flow
   still works end to end against the new image.
2. **Full audited multi-scene verification** (all scenes, both agents, all
   three bankers via `scripts/19-run-scene7-persona-demo.sh --aud`) was not
   completed — see "Automating parrot's interactive TUI" above. The pty
   sizing fix (`stty_init "rows 50 columns 200"`) is proven and reusable;
   what's still open is whether running three personas' `parrot` sessions
   concurrently causes a genuine stall or whether that was a red herring.
   Worth a fuller pass later if complete scene-by-scene coverage is wanted,
   same shape as the 0.1.2 doc's own "Scenes 1–6" section.
3. **Re-wire MLflow tracing onto the current (0.1.8) `bob` sandboxes** and
   test whether `[openshell.supervisor.network].additional_ca_bundle`
   (v0.1.7) actually resolves the original TLS-trust blocker documented on
   the `mlflow-tracing-claude-sandbox` branch — tracked as a separate,
   explicit follow-up on this branch, not folded into the version-bump
   scope above.
4. **Merging `v0.1.8-upgrade` into `main`** is deliberately deferred — a
   colleague may be actively working from `main`, so this branch stands on
   its own as the "confirmed working on 0.1.8" reference until that's
   coordinated.
5. `token-exchange-profile.yaml`'s missing `auth_style` (noted above) is a
   pre-existing gap in an already-marked-draft file, not touched here.
