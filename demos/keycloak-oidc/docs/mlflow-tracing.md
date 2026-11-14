# MLflow tracing for Claude Code sandboxes

Live Claude Code *session* tracing (tool-call sequences, token counts,
duration, status per OTel span) into RHOAI's MLflow, for a single sandbox
(e.g. `claude-charlie`). This is a different signal from the two MLflow/OTel
paths this repo already has:

- `scripts/lib-otel-env.sh` ships raw OTel spans to `audit-collector`, an
  OTel collector, for the audit dashboard — no MLflow involved.
- `evalhub-redteam.md` wires EvalHub/Garak **batch red-team eval scores**
  into MLflow — adversarial-probe results, not live agent-session traces.

This doc follows the recipe in
[agentic-starter-kits' claude-code README](https://github.com/red-hat-data-services/agentic-starter-kits/blob/main/agents/claude-code/README.md#mlflow-tracing-optional),
adapted for this repo's `openshell sandbox`-based provisioning model (that
guide assumes a static `Deployment` with its own long-running entrypoint;
OpenShell sandboxes have neither, so the one-time setup it does automatically
at container start has to be done once by hand per sandbox instead — see
Step 4 below).

## What's confirmed live (sandbox341, namespace `keycloak-oidc-demo`)

- Every sandbox in a namespace shares one ServiceAccount, `openshell-sandbox`
  (confirmed: alice's, bob's, and charlie's sandbox pods all run as it) — so
  any RBAC grant below is namespace-wide, not per-banker.
- The sandbox's runtime network policy is fail-closed with no PyPI egress:
  `curl`/DNS to `pypi.org` times out from inside a running sandbox pod. mlflow
  must be installed at **build** time (Step 1), not via a runtime `pip
  install`.
- The base image ("Hummingbird OS") has no `dnf`/`microdnf` — no OS package
  manager at all. `python3` has the `ensurepip` module but pip itself isn't
  bootstrapped (`python3 -m pip install` fails with "No module named pip"
  until `python3 -m ensurepip --upgrade` runs first — confirmed on the build
  host, carlos-kenshin).
- RHOAI's MLflow is already deployed: CR `mlflow` in
  `redhat-ods-applications`, internal URL
  `https://mlflow.redhat-ods-applications.svc:8443/mlflow`, server version
  3.14.0. The matching RBAC role is `mlflow-operator-mlflow-integration`
  (RHOAI's own prefix — the upstream guide just calls it `mlflow-integration`).
  EvalHub already binds this same ClusterRole to its own controller SA via a
  `ClusterRoleBinding`, which is the precedent this repo's RBAC chart copies.
- No CA bundle is mounted in sandbox pods, and OpenShell's `policy set`
  (filesystem/network/process only) has no way to add one. Pragmatic choice
  for now: `MLFLOW_TRACKING_INSECURE_TLS=true` instead of wiring a service-ca
  mount.
- MLflow client version matters independently of the server version: this
  image pins `mlflow>=3.12,<3.13` deliberately, to stay on the simpler Python
  stop-hook path (`mlflow autolog claude`) rather than MLflow 3.14+'s
  Claude Code plugin path (TypeScript SDK, `NODE_EXTRA_CA_CERTS`, and per
  upstream docs a plugin that currently needs building from source). The 3.12
  client hook works fine against this cluster's 3.14.0 server.

## Steps

1. **Bake MLflow into the image** — done:
   `images/claude-sandbox/Containerfile` now installs `mlflow>=3.12,<3.13`
   via `python3 -m pip install` as part of the build. Build and push per the
   comment at the top of that file (new tag: `0.4.1`). Deliberately *not*
   `0.4.0-mlflow` — that was this doc's first attempt, reverted: mlflow is a
   baseline feature of this image now, not a flagged variant alongside a
   plain `0.4.0` you'd choose between, so it gets a normal version bump
   instead of a feature-named tag (same convention `util/session-auditor`'s
   own images already use — bump on content change, never encode what
   changed into the tag itself).
2. **Add an egress rule** — done: `policies/values.yaml` gained
   `mlflowTrackingHost` / `mlflowTrackingPort` (both optional, default port
   `8443`); `policies/templates/policy.yaml` renders an `allow_mlflow`
   network-policy group when `mlflowTrackingHost` is set, granting
   `/usr/local/bin/claude`, `/usr/bin/node`, and `/usr/sbin/python3` access to
   it (the Stop-hook runs as a child process of one of these).
3. **Grant RBAC** — done: `mlflow-sandbox-rbac/` is a new chart (rendered
   via `helm template`, applied via `oc apply -f -`, same convention as
   `policies/`) producing a `ClusterRoleBinding` from the namespace's
   `openshell-sandbox` SA to `mlflow-operator-mlflow-integration`. Apply with:

   ```bash
   helm template mlflow-sandbox-rbac mlflow-sandbox-rbac \
     --set openshellNamespace=keycloak-oidc-demo | oc apply -f -
   ```

4. **Delete and recreate the target sandbox** — not yet done. Image and
   policy are both creation-time-only (confirmed by existing comments in
   `scripts/15-provision-claude-sandbox.sh`), so an already-provisioned
   sandbox (e.g. `claude-charlie`) must be deleted and re-provisioned with
   `CLAUDE_IMAGE` pointing at the new `0.4.1` tag, and with
   `mlflowTrackingHost=mlflow.redhat-ods-applications.svc.cluster.local`
   threaded into the `helm template policies` call inside that script. This
   drops whatever's on the sandbox's current `/sandbox` PVC.
5. **One-time hook registration** — not yet done. After the sandbox exists:

   ```bash
   openshell sandbox exec -n claude-charlie --workspace charlie -- \
     mlflow autolog claude -d /sandbox/.claude \
       -u https://mlflow.redhat-ods-applications.svc:8443/mlflow \
       -n claude-code-traces
   ```

   then inject `MLFLOW_TRACKING_AUTH=kubernetes-namespaced`,
   `MLFLOW_WORKSPACE=<namespace>`, `MLFLOW_TRACKING_INSECURE_TLS=true`, and
   the pod's own SA token into `/sandbox/.claude/settings.json`'s `"env"`
   block — the same injection
   [agentic-starter-kits' `entrypoint.sh`](https://github.com/red-hat-data-services/agentic-starter-kits/blob/main/agents/claude-code/deployment/entrypoint.sh)
   does automatically on every pod start; OpenShell sandboxes have no
   equivalent entrypoint, so this has to run by hand, once, per sandbox.
6. **Verify** — not yet done. Run a prompt in the sandbox, then check the
   MLflow UI (`https://rh-ai.apps.<cluster-domain>/mlflow`) under workspace
   `<namespace>`, experiment `claude-code-traces`.

## Known limitation

The RBAC grant in Step 3 is namespace-wide (every sandbox shares
`openshell-sandbox`), not scoped to one banker's sandbox. If per-banker MLflow
isolation ever matters, OpenShell would need a per-sandbox ServiceAccount
before this chart could be scoped any tighter.

## `claude-audit` needs a different Step 1/5 — baked, not dynamic

`images/claude-audit/Containerfile` layers `session-auditor` on top of
`claude-sandbox` and bakes `/etc/claude-code/managed-settings.json` with
`"allowManagedHooksOnly": true` (see `util/session-auditor/README.md`) —
**on purpose**, so the sandbox's own user can't disable the audit hooks.
That lockdown has a side effect discovered while doing this: it also blocks
any hook written to the sandbox's own `/sandbox/.claude/settings.json`,
which is exactly where Step 5's `mlflow autolog claude` command registers
its Stop-hook for the plain `claude-sandbox` image. Confirmed by reading the
managed-settings precedence rules the session-auditor README already
documents (not independently live-tested against MLflow specifically) —
treat this as the same class of silent failure the session-auditor README
warns about for its own hooks: sandbox stays `Ready`, agent answers
normally, no trace ever shows up in MLflow, no error anywhere.

So for `claude-audit`, the hook has to be baked into
`managed-settings.json` itself, as a second entry in the `Stop` array
alongside `session-auditor`'s:

```json
"Stop": [
  { "matcher": "*", "hooks": [{ "type": "command", "command": "/usr/local/bin/session-auditor" }] },
  { "matcher": "*", "hooks": [{ "type": "command", "command": "mlflow autolog claude stop-hook" }] }
]
```

This only registers the hook — it still needs `MLFLOW_CLAUDE_TRACING_ENABLED=true`,
`MLFLOW_TRACKING_URI`, `MLFLOW_EXPERIMENT_NAME`, `MLFLOW_TRACKING_AUTH`,
`MLFLOW_WORKSPACE`, and `MLFLOW_TRACKING_INSECURE_TLS` present in the
environment the hook's parent `claude` process runs in to do anything. Per
`util/session-auditor/README.md`'s own documented pattern for
`AUDITOR_LLM_BASE_URL`/`AUDITOR_ANTHROPIC_MODEL` ("child processes inherit
them"), these should be passed the same way: `--env` flags on the
`sandbox exec ... claude` call, same mechanism `scripts/lib-otel-env.sh`
already uses for OTel — not written into `settings.json`'s `"env"` block
(that's `/etc`, read-only, so it would have to be baked in anyway, coupling
the image to one specific MLflow deployment). **Not yet live-verified** that
a Stop-hook subprocess actually inherits exec-time `--env` the same way the
main `claude` process's own tool calls do — confirm this when doing Step 6
for an audited sandbox.

Also done, build-side: `images/claude-audit/Containerfile`'s `FROM` now
points at `quay.io/atarazana/claude-sandbox:0.4.1` (the mlflow-baseline
revision) instead of `:0.4.0`, so it inherits the pip-installed `mlflow`
client with no extra `RUN` needed. Built and pushed
`quay.io/atarazana/claude-audit:0.4.1-0.1.5` (tag convention per
`util/session-auditor/Makefile`: base image tag + crate version — unchanged
crate version since nothing about the binary itself changed, only its base
image). Built on carlos-kenshin inside a `clux/muslrust:stable` container
(not directly on the host — it's an immutable Fedora Atomic system with no
`cc` linker at all, confirmed live) using the pinned `session-auditor` 0.1.5
source already in this repo, since GitHub auth was unavailable locally to
pull the prebuilt release binary instead.

An earlier pass briefly tagged these `0.4.0-mlflow` /
`0.4.0-mlflow-0.1.5` before settling on the scheme above — those two quay
tags still exist (pushed, then superseded) and should be deleted from
quay.io/atarazana to avoid confusion, but weren't as of this writing.
