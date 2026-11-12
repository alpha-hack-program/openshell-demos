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
  manager at all, just `python3` with `pip` already importable as a module.
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
   comment at the top of that file (new tag: `0.4.0-mlflow`).
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
   `CLAUDE_IMAGE` pointing at the new `-mlflow` tag, and with
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
