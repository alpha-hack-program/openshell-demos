# MLflow tracing for Claude Code sandboxes

Live Claude Code *session* tracing (tool-call sequences, token counts,
duration, status per OTel span) into RHOAI's MLflow, for Claude Code
sandboxes (`claude-<user>` and `aud-claude-<user>`). This is a different
signal from the two MLflow/OTel paths this repo already has:

- `scripts/lib-otel-env.sh` ships raw OTel spans to `audit-collector`, an
  OTel collector, for the audit dashboard — no MLflow involved.
- `evalhub-redteam.md` wires EvalHub/Garak **batch red-team eval scores**
  into MLflow — adversarial-probe results, not live agent-session traces.

This doc follows the recipe in
[agentic-starter-kits' claude-code README](https://github.com/red-hat-data-services/agentic-starter-kits/blob/main/agents/claude-code/README.md#mlflow-tracing-optional),
adapted for this repo's `openshell sandbox`-based provisioning model (that
guide assumes a static `Deployment` with its own long-running entrypoint;
OpenShell sandboxes have neither, so the one-time setup it does automatically
at container start is instead run once per sandbox by
`scripts/15-provision-claude-sandbox.sh` — see "Steps" below).

**Status: fully working and automated, verified live end to end on
OpenShell 0.1.8** (sandbox341, 2026-11-14) — a real trace, with the actual
prompt/response pair, confirmed in MLflow for both a plain `claude-bob`
sandbox and an audited `aud-claude-bob` sandbox, provisioned by the
unmodified `scripts/15`/`17`/`19`. Getting there required finding and
fixing three separate bugs (two in this repo, one blocked entirely on the
OpenShell version) — see below for what they were and why they mattered,
since the symptom for all three was identical and silent: "sandbox stays
`Ready`, agent answers normally, no trace ever shows up in MLflow, no
error anywhere."

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
  `https://mlflow.redhat-ods-applications.svc.cluster.local:8443/mlflow`,
  server version 3.14.0. The matching RBAC role is
  `mlflow-operator-mlflow-integration` (RHOAI's own prefix — the upstream
  guide just calls it `mlflow-integration`). EvalHub already binds this same
  ClusterRole to its own controller SA via a `ClusterRoleBinding`, which is
  the precedent this repo's RBAC chart copies.
- No CA bundle is mounted in sandbox pods, and OpenShell's `policy set`
  (filesystem/network/process only) has no way to add one. Pragmatic choice:
  `MLFLOW_TRACKING_INSECURE_TLS=true` on the client side instead of wiring a
  service-ca mount into the sandbox — this is about the *sandbox's own*
  trust of the supervisor's MITM-reissued TLS cert (every outbound
  connection from a sandbox is transparently intercepted and re-signed by
  "OpenShell Sandbox CA" for policy enforcement), a separate, already-
  accepted tradeoff from the supervisor's *own* backend trust covered next.
- MLflow client version matters independently of the server version: this
  image pins `mlflow>=3.12,<3.13` deliberately, to stay on the simpler Python
  stop-hook path (`mlflow autolog claude`) rather than MLflow 3.14+'s
  Claude Code plugin path (TypeScript SDK, `NODE_EXTRA_CA_CERTS`, and per
  upstream docs a plugin that currently needs building from source). The 3.12
  client hook works fine against this cluster's 3.14.0 server.
- RHOAI's MLflow requires a workspace-context header (`X-MLFLOW-WORKSPACE`)
  on every API call once multi-tenant workspaces are enabled — without it,
  requests fail with a generic `400 INVALID_PARAMETER_VALUE: Workspace
  context is required for this request`, easy to mistake for a malformed
  request rather than a missing header. The MLflow Python client sets this
  automatically from the `MLFLOW_WORKSPACE` environment variable (see
  `mlflow/utils/workspace_context.py`'s `get_request_workspace()`) — set it
  to the OpenShell namespace (`OPENSHELL_NAMESPACE`), not an arbitrary name.

## The original blocker: the supervisor's own backend TLS trust — fixed by upgrading to OpenShell 0.1.8

Before this fix, every attempt died the same way: the sandbox's outbound
HTTPS connection to MLflow was correctly allowed by policy, and the
supervisor correctly intercepted it (confirmed via `curl -v` showing a
cert issued by "OpenShell Sandbox CA"), but the supervisor's own **separate**
backend connection to the real MLflow server then failed — confirmed in the
supervisor pod's OCSF logs:

```
NET:OPEN [INFO] ALLOWED /usr/bin/curl(0) -> mlflow...:8443 [policy:allow_mlflow engine:opa]
NET:FAIL [LOW] mlflow.redhat-ods-applications.svc.cluster.local:8443
```

Root cause: RHOAI's MLflow `Service` uses OpenShift's own service-serving
certificate mechanism (`service.beta.openshift.io/serving-cert-secret-name:
mlflow-tls`, signed by `openshift-service-serving-signer`) — an internal CA
the supervisor's own backend HTTP client has no reason to trust by default.
**OpenShell v0.1.7 added exactly the missing piece**:
`[openshell.supervisor.network].additional_ca_bundle`, exposed in the Helm
chart as `supervisor.network.additionalCaConfigMapName`. Applied live:

```bash
oc get secret signing-key -n openshift-service-ca \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > /tmp/service-ca.crt
oc create configmap mlflow-destination-ca -n keycloak-oidc-demo \
  --from-file=ca.crt=/tmp/service-ca.crt

helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version 0.1.8 --namespace keycloak-oidc-demo \
  -f helm/values-certmanager.yaml \
  <...the usual --set overrides from "2a. Helm install" in the README...> \
  --set "supervisor.network.additionalCaConfigMapName=mlflow-destination-ca"
```

Per the chart's own docs, this needs a gateway restart (`helm upgrade`
restarts it automatically if the rendered pod template changes) and a
sandbox restart/recreation to pick up the new supervisor-side trust roots —
already-running sandboxes keep their old trust until recreated. Confirmed
live: after the upgrade and a fresh sandbox, the exact same supervisor logs
flipped from `NET:FAIL` to real success:

```
NET:OPEN [INFO] ALLOWED /usr/bin/python3.14(0) -> mlflow.redhat-ods-applications.svc.cluster.local:8443 [policy:_provider_mlflow engine:opa]
HTTP:GET [INFO] ALLOWED GET https://mlflow.../mlflow/api/2.0/mlflow/experiments/get-by-name [policy:_provider_mlflow engine:l7]
```

**This means this feature is unavailable before OpenShell 0.1.7.** On an
older gateway there is no workaround — the supervisor's backend leg simply
cannot be made to trust a private CA.

## A real repo bug, now fixed: `mlflow autolog claude -d` path resolution

`mlflow autolog claude -d <dir>` appends `.claude/settings.json` to
whatever directory you pass it — this doc's own first version got this
wrong (`-d /sandbox/.claude`), which silently wrote the Stop hook to
**`/sandbox/.claude/.claude/settings.json`**, a path Claude Code never
reads. The command still printed "✅ Claude Code hooks configured" and
exited 0 — there is no error anywhere, the hook just never fires. Confirmed
by reading `mlflow/claude_code/cli.py`'s source directly:
`target_dir = Path(directory).resolve(); claude_dir = target_dir /
".claude"`. **The correct directory is the sandbox's home, `/sandbox`**,
not `/sandbox/.claude`. `scripts/15-provision-claude-sandbox.sh` now runs
this registration automatically with the correct flag.

## Steps — now fully automated

1. **Bake MLflow into the image** — done: `images/claude-sandbox/Containerfile`
   installs `mlflow>=3.12,<3.13` via `python3 -m pip install` as part of the
   build (current tag: `0.4.2` — no separate `-mlflow` tag, mlflow is a
   baseline feature of this image, not a flagged variant).
2. **Egress policy grant** — done, automatic: `scripts/15-provision-claude-sandbox.sh`
   threads `MLFLOW_TRACKING_HOST`/`MLFLOW_TRACKING_PORT` (from `.env`) into
   the `policies` chart's `mlflowTrackingHost`/`mlflowTrackingPort` values,
   rendering an `allow_mlflow` network-policy group whenever
   `MLFLOW_TRACKING_HOST` is set — omitted entirely when it's unset.
3. **RBAC grant** — done, one-time, cluster state (not re-applied by any
   script): `mlflow-sandbox-rbac/` renders a `ClusterRoleBinding` from the
   namespace's `openshell-sandbox` SA to `mlflow-operator-mlflow-integration`:

   ```bash
   helm template mlflow-sandbox-rbac mlflow-sandbox-rbac \
     --set openshellNamespace=keycloak-oidc-demo | oc apply -f -
   ```

4. **Provider creation/attachment** — done, automatic:
   `scripts/15-provision-claude-sandbox.sh` imports `providers/mlflow-profile.yaml`
   (substituting `<mlflow-host>`/`<mlflow-port>`), creates a per-workspace
   `mlflow` provider from `MLFLOW_TRACKING_TOKEN` (see "Minting and
   refreshing the token" below), and attaches it at `sandbox create` time —
   plus a `sandbox provider attach` fallback for already-existing sandboxes,
   mirroring exactly how `byo-claude`/`user-<id>` are already handled. All
   gated on `MLFLOW_TRACKING_HOST` being set; a complete no-op otherwise.
5. **One-time hook registration** — done, automatic, for the plain
   (non-audited) image: `scripts/15` runs
   `mlflow autolog claude -d /sandbox -u https://<host>:<port>/mlflow -n claude-code-traces`
   right after the sandbox is created (idempotent — safe to re-run). For
   the audited image (`claude-audit`), the hook is baked into
   `managed-settings.json` at build time instead — see "`claude-audit`
   needs a different Step 1/5" below; running the dynamic registration
   against an audited sandbox is a harmless no-op (see that section for
   why).
6. **Runtime env vars at exec time** — done, automatic:
   `scripts/lib-mlflow-env.sh`'s `mlflow_env_args` (sourced by
   `scripts/19-run-scene7-persona-demo.sh`) adds
   `MLFLOW_CLAUDE_TRACING_ENABLED`, `MLFLOW_TRACKING_URI`,
   `MLFLOW_EXPERIMENT_NAME`, `MLFLOW_WORKSPACE`, and
   `MLFLOW_TRACKING_INSECURE_TLS` via `--env` to every Claude Code turn
   (Claude-only — there is no Codex MLflow integration). Harmless empty
   array when `MLFLOW_TRACKING_HOST` is unset. For manual
   `openshell sandbox exec`/`parrot` calls outside script 19, pass these
   five yourself the same way.
7. **Verify** — confirmed live via the MLflow REST API directly (the UI
   path, `https://rh-ai.apps.<cluster-domain>/mlflow`, workspace
   `keycloak-oidc-demo`, experiment `claude-code-traces`, was not
   independently re-checked this round, only the API):

   ```bash
   openshell sandbox exec -n claude-bob --workspace bob -- python3 -c '
   import os, requests, urllib3
   urllib3.disable_warnings()
   token = os.environ["MLFLOW_TRACKING_TOKEN"]
   headers = {"Authorization": f"Bearer {token}", "X-MLFLOW-WORKSPACE": "keycloak-oidc-demo"}
   r = requests.post(
       "https://mlflow.redhat-ods-applications.svc.cluster.local:8443/mlflow/api/3.0/mlflow/traces/search",
       headers=headers,
       json={"locations": [{"type": "MLFLOW_EXPERIMENT", "mlflow_experiment": {"experiment_id": "1"}}], "max_results": 5},
       verify=False,
   )
   print(r.status_code, r.text[:2000])
   '
   ```

## Minting and refreshing the token

`MLFLOW_TRACKING_TOKEN` (in `.env`) is a **manually-minted Kubernetes
ServiceAccount bearer token** for `openshell-sandbox` — not an
auto-discovered in-pod token (`MLFLOW_TRACKING_AUTH=kubernetes-namespaced`
cannot work here: OpenShell sandbox pods don't mount a ServiceAccount token
at all, confirmed live, `/var/run/secrets/kubernetes.io/serviceaccount/`
doesn't exist in the pod). RHOAI's MLflow server authenticates **any**
bearer token the same way regardless of how the client obtained it: it
performs a Kubernetes `TokenReview` against the token it's handed. The
`mlflow-sandbox-rbac` grant exists precisely so that validation succeeds no
matter how the SA's token was minted.

**Mint one:**

```bash
oc create token openshell-sandbox -n keycloak-oidc-demo --duration=8760h
```

`--duration=8760h` (365 days) was used here — a modern, bound
(`TokenRequest`-API-style) token, genuinely time-limited by design, not an
old-style eternal Secret token. It expires roughly a year after minting;
after that, every MLflow call fails with
`401 UNAUTHENTICATED: Authentication with the Kubernetes API failed. The
provided token may be invalid or expired` — a message that's easy to
mistake for a wrong token value rather than simple expiry.

**Apply a new one** to every banker's `mlflow` provider (one provider per
workspace, same token value, since the RBAC grant is namespace-wide):

```bash
for USER_ID in alice bob charlie; do
  openshell provider update mlflow --workspace "$USER_ID" \
    --credential "MLFLOW_TRACKING_TOKEN=<new token>" --wait
done
```

**Is there a built-in refresh strategy for this?** No, and there isn't one
that could fit without an upstream OpenShell code change. Checked
`openshell provider refresh configure --strategy`: the CLI only exposes
four values — `oauth2-refresh-token`, `oauth2-client-credentials`,
`google-service-account-jwt`, `aws-sts-assume-role` — and the underlying
gateway binary's `ProviderCredentialRefreshStrategy` enum (compiled into
the Rust binary via protobuf, not a runtime-pluggable interface) only adds
two more: `Static` and `External`, neither of which are settable via this
CLI version and neither of which actually execute a refresh — they're
bookkeeping states meaning "nothing auto-refreshes this; something else
keeps it current." Kubernetes bound ServiceAccount tokens also have no
OAuth-style refresh_token concept of their own to plug into any of the
four real strategies — in a normal pod, kubelet itself re-requests a fresh
projected token shortly before expiry, which doesn't apply here since
nothing mounts this token into the sandbox pod automatically (that
limitation is the entire reason this manual-token workaround exists in the
first place).

Practical options, in order of effort:

1. **Long duration + manual renewal** (what's done here) — simplest, fine
   for a demo, needs a calendar reminder before the ~1-year expiry.
2. **A small external refresh job** — a `CronJob` (or an admin-run,
   numbered script following this repo's own convention) that periodically
   re-runs `oc create token` and `openshell provider update ... --wait` for
   each banker. This is exactly what the `External` strategy label is
   naming, conceptually — "refreshed by something outside the gateway" —
   just not a mechanism OpenShell's own refresh engine executes for you.
   Not built in this repo this round; tracked as follow-up.
3. **A genuine new refresh strategy upstream** (e.g.
   `kubernetes-token-request`, calling the Kubernetes `TokenRequest` API
   the same way `aws-sts-assume-role` calls AWS STS) would be the "proper"
   fix, and is a reasonable, scoped feature request against
   `NVIDIA/OpenShell` — the pattern already exists for three other cloud
   identity providers, Kubernetes bound tokens are a very standard
   equivalent many operators would want, and the credential-issuer crate's
   `minter.rs` already has a clean per-strategy dispatch point to extend.
   Not something this repo can do on its own, though — it needs a change
   to OpenShell's own compiled gateway binary, not just this demo's
   scripts/config.

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
That lockdown has a side effect: it also makes Claude Code ignore any hook
found in the sandbox's own, lower-precedence `/sandbox/.claude/settings.json`
— exactly where the dynamic `mlflow autolog claude -d /sandbox ...`
registration writes for the plain `claude-sandbox` image. Running that
registration against an audited sandbox is therefore a harmless no-op (the
hooks get written, just never consulted), not an error — consistent with
the same class of silent failure the session-auditor README already warns
about for its own hooks.

So for `claude-audit`, the hook is baked into `managed-settings.json`
itself, as a second entry in the `Stop` array alongside `session-auditor`'s:

```json
"Stop": [
  { "matcher": "*", "hooks": [{ "type": "command", "command": "/usr/local/bin/session-auditor" }] },
  { "matcher": "*", "hooks": [{ "type": "command", "command": "mlflow autolog claude stop-hook" }] }
]
```

This only registers the hook — it still needs `MLFLOW_CLAUDE_TRACING_ENABLED=true`,
`MLFLOW_TRACKING_URI`, `MLFLOW_EXPERIMENT_NAME`, `MLFLOW_WORKSPACE`, and
`MLFLOW_TRACKING_INSECURE_TLS` present in the environment the hook's parent
`claude` process runs in to do anything (`MLFLOW_TRACKING_AUTH` is
deliberately **not** one of these — see "Minting and refreshing the token"
above for why `kubernetes-namespaced` can never work in this environment).
Per `util/session-auditor/README.md`'s own documented pattern for
`AUDITOR_LLM_BASE_URL`/`AUDITOR_ANTHROPIC_MODEL` ("child processes inherit
them"), these are passed the same way: `--env` flags on the
`sandbox exec ... claude` call (`scripts/lib-mlflow-env.sh`, sourced by
`scripts/19-run-scene7-persona-demo.sh`) — not written into
`settings.json`'s `"env"` block (that's `/etc`, read-only, so it would have
to be baked in anyway, coupling the image to one specific MLflow
deployment). **Confirmed live** that a Stop-hook subprocess does inherit
exec-time `--env` the same way the main `claude` process's own tool calls
do — a real trace from `aud-claude-bob` (prompt "What is 9+9?", response
"18") landed in MLflow using exactly this mechanism.

Build-side: `images/claude-audit/Containerfile`'s `FROM` points at
`quay.io/atarazana/claude-sandbox:0.4.2` (the mlflow-baseline revision), so
it inherits the pip-installed `mlflow` client with no extra `RUN` needed.
