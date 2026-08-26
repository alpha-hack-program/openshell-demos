# EvalHub Red-Team Extension — keycloak-oidc demo

Status: **Validated end-to-end on a live cluster**, most recently
2026-08-26 (Claude Code path, current Meridian naming — see
[Annex G](#g-validated-findings-log)). Originally drafted as a
design/planning doc — see
[`evalhub-redteam-orig.md`](evalhub-redteam-orig.md) for that history if
needed; this file is now the authoritative, current guide.

> **Naming note:** the instructional sections below (Demo steps, Annex D's
> roles/naming) use the current Part I naming — `alice`/`bob`/`charlie`,
> and `mcp-portfolio` instead of the old, removed `mcp-server-a` (an
> "eligibility engine" unrelated to the current banking domain).
> [Annex G — Validated findings log](#g-validated-findings-log) is mixed:
> its earlier entries (2026-08-18) are a literal historical record under
> the old `user1`/`mcp-server-a` naming, predating both the Meridian
> re-theme and per-banker workspace isolation — left as-is rather than
> rewritten into a result that naming never actually produced. Its latest
> entry (2026-08-26) re-confirms the Claude Code path end to end under the
> current naming and is what the Demo steps section above now reflects.
> The Codex variant has not been re-run under current naming.

## Table of contents

- [Overview](#overview)
  - [Purpose](#purpose)
  - [Architecture](#architecture)
  - [Key insight](#key-insight)
  - [How to follow this guide](#how-to-follow-this-guide)
- [Prerequisites (admin, one-time)](#prerequisites-admin-one-time)
- [Demo steps — Claude Code (recommended)](#demo-steps--claude-code-recommended)
- [Demo steps — Codex (optional)](#demo-steps--codex-optional)
- [Annexes](#annexes)
  - [A. Alternative: Approach B — custom sandbox image](#a-alternative-approach-b--custom-sandbox-image)
  - [B. Troubleshooting](#b-troubleshooting)
  - [C. Resolved design decisions](#c-resolved-design-decisions)
  - [D. Roles and responsibilities](#d-roles-and-responsibilities)
  - [E. EvalHub integration background](#e-evalhub-integration-background)
  - [F. Viewing results in the RHOAI dashboard / MLflow](#f-viewing-results-in-the-rhoai-dashboard--mlflow)
  - [G. Validated findings log](#g-validated-findings-log)
  - [H. Open items](#h-open-items)
  - [I. Proposed file additions](#i-proposed-file-additions)
  - [J. Phases](#j-phases)
  - [K. References](#k-references)

## Overview

### Purpose

Run repeatable, auditable red-team evaluations against AI agents (Codex /
Claude Code) running inside OpenShell sandboxes — using EvalHub as the
orchestrator (via RHOAI UI or CLI), Garak as the adversarial probe engine,
and a Rust proxy (`agent-proxy`) uploaded into the sandbox to bridge
Garak's OpenAI-compatible API expectations to the CLI-based agent.

> **Claude Code is the primary path for this demo; Codex is optional.**
> Against the demo's default DeepSeek BYO backend, Codex can only exercise
> model-only red-team probes — DeepSeek rejects the `namespace` tool type
> Codex uses for MCP, so Codex + MCP evaluations don't work here (see
> [Annex B — Troubleshooting](#b-troubleshooting)). Codex + MCP only works
> against an on-cluster vLLM **≥0.25.0** (upstream; RHOAI 3.4.x ships
> 0.18.0, which is too old — see `docs/inference-api-compatibility.md`).
> Claude Code's Anthropic Messages API has no such restriction and is
> validated end-to-end against DeepSeek, including MCP tool use (see
> [Annex G — Validated findings log](#g-validated-findings-log)).

### Architecture

![EvalHub red-team evaluation architecture](diagrams/evalhub-redteam-architecture.svg)

<details>
<summary>ASCII source</summary>

```
  RHOAI UI / evalhub CLI
         │
         ▼
  EvalHub Server ───► Garak Job (K8s Job)
         │                    │
         │                    ▼
         │         garak-envoy (real Route)
         │         rewrites Host from URL path
         │         /route/<host-key>/...
         │                    │
         │                    ▼
         │         OpenShell gateway (Host-header routed)
         │                    │
         │  ┌─ OpenShell sandbox (stock image) ───────────┐
         │  │                                              │
         │  │  ┌──────────────┐                            │
         │  │  │ agent-proxy  │◄── :8080 (OpenAI-compat)   │
         │  │  │ (Rust, up-   │                            │
         │  │  │  loaded bin) │                            │
         │  │  └──────┬───────┘                            │
         │  │         │ shells out to                      │
         │  │         ▼                                    │
         │  │  codex "..."                                 │
         │  │         │                                    │
         │  │         ├──► MCP servers (Envoy)             │
         │  │         ├──► network policies                │
         │  │         └──► binary permissions              │
         │  └──────────────────────────────────────────────┘
         │
         ▼
  MLflow ◄── metrics + attack logs
```

</details>

**Why `garak-envoy`?** Garak's OpenAI-compatible client can't set a custom
`Host` header, but `openshell service expose` routes purely by `Host`
header — multiple sandboxes share one gateway Route, disambiguated only
that way. Without a workaround, a Garak Job can only ever reach the
gateway's *default* vhost (confirmed on a live cluster:
`openai.NotFoundError: Error code: 404`). `garak-envoy` sits on its own
real, DNS-resolvable Route and extracts the routing key from the request
*path* instead, rewriting it to the `Host` header the gateway needs — see
[Annex E — EvalHub integration background](#e-evalhub-integration-background)
(the "CONFIRMED BROKEN" note) for the full root-cause writeup.

### Key insight

The agent-proxy runs **inside** the sandbox, not in front of it. Garak's
adversarial probes hit the agent in the exact same environment a real user
would have — network policies, binary permissions, MCP server RBAC are all
live, not simulated. The proxy is exposed only on demand via
`openshell service expose`.

### How to follow this guide

**Note on roles:** In production, red-team evaluation is a secops
function, run automatically by an automation/service-account identity —
never a human, and never the target user themselves (self-auditing is a
conflict of interest). **`alice` in the steps below names *whose provider
and MCP roles get attached to the sandbox*, not who issues the
commands.** Every command — sandbox creation, provider attachment, policy
updates, proxy startup, eval submission — runs from **one continuous
admin/secops session**. This works because Providers v2 injects
credentials per *attached* provider and MCP RBAC checks whatever JWT that
provider supplies at request time — neither cares who ran `sandbox
create`. So the sandbox gets exactly `alice`'s security context (her MCP
roles, her credentials) even though an admin/secops identity created and
drove it end to end. See
[Annex D — Roles and responsibilities](#d-roles-and-responsibilities) for
the full production model (representative profiles instead of named
users, automated loops across all of them).

## Prerequisites (admin, one-time)

1. **Enable the TrustyAI component** — EvalHub is deployed by
   the TrustyAI Operator, which ships with RHOAI but is disabled by
   default. Enable it in the DataScienceCluster, then wait for the
   operator and CRDs to appear:

   ```bash
   # Enable TrustyAI (skip if already Managed)
   oc patch datasciencecluster default-dsc --type=merge \
     -p '{"spec":{"components":{"trustyai":{"managementState":"Managed"}}}}'

   # Wait for the TrustyAI operator pod
   oc -n redhat-ods-applications get pods -l control-plane=controller-manager,app.kubernetes.io/part-of=trustyai --watch
   # If that label selector doesn't match on your RHOAI version, just watch
   # for a pod named trustyai-service-operator-controller-manager-* instead —
   # confirmed live (RHOAI 3.4.3): the pod name doesn't match the label this
   # guide originally assumed (app=trustyai-operator matches nothing).
   # Ctrl-C once a pod shows Running

   # Confirm the EvalHub CRD exists
   oc api-resources | grep -i eval
   ```

   Also confirm that KServe is in `RawDeployment` mode (required by
   EvalHub):

   ```bash
   oc get datasciencecluster default-dsc \
     -o jsonpath='{.spec.components.kserve.rawDeploymentServiceConfig}'
   # Should print "Headless"
   ```

2. **Deploy PostgreSQL** — EvalHub needs a PostgreSQL database.
   Deploy one in the EvalHub namespace (or point to an existing instance).
   A minimal ephemeral deployment for demo purposes:

   ```bash
   EVALHUB_NAMESPACE="evalhub"
   oc create namespace "$EVALHUB_NAMESPACE" 2>/dev/null || true

   oc -n "$EVALHUB_NAMESPACE" new-app \
     --name=evalhub-db \
     -e POSTGRESQL_USER=evalhub \
     -e POSTGRESQL_PASSWORD=evalhub-demo-password \
     -e POSTGRESQL_DATABASE=evalhub \
     --image-stream="openshift/postgresql:15-el9"

   oc -n "$EVALHUB_NAMESPACE" rollout status deployment/evalhub-db
   ```

3. **Create the EvalHub CR — with MLflow tracking wired in from the
   start.** EvalHub has no native RHOAI dashboard view of its own (the
   dashboard's model evaluation UI is LM-Eval-only) — MLflow is the only
   UI-capable path for Garak results, so this demo treats it as a required
   prerequisite, prepared before the CR exists, rather than an optional
   patch applied afterward.

   Confirm the cluster's native RHOAI MLflow instance is present
   (deploying MLflow itself is outside this demo's scope — enable it first
   if this comes back empty):

   ```bash
   oc get mlflow -A
   ```

   If present, the TrustyAI operator has likely already pre-wired most of
   the integration against it (CA cert, workspace, and a
   projected-ServiceAccount-token mount with matching RBAC) — the only
   thing this demo needs to set explicitly is `MLFLOW_TRACKING_URI`:

   ```bash
   EVALHUB_NAMESPACE="evalhub"
   MLFLOW_INTERNAL_URL=$(oc get mlflow mlflow -n redhat-ods-applications \
     -o jsonpath='{.status.address.url}')

   # Create the PostgreSQL connection Secret (EvalHub expects a db-url key)
   oc -n "$EVALHUB_NAMESPACE" apply -f - <<'EOF'
   apiVersion: v1
   kind: Secret
   metadata:
     name: evalhub-db-credentials
   type: Opaque
   stringData:
     db-url: "postgresql://evalhub:evalhub-demo-password@evalhub-db:5432/evalhub"
   EOF

   # Create the EvalHub CR with the garak provider and MLflow tracking enabled
   oc -n "$EVALHUB_NAMESPACE" apply -f - <<EOF
   apiVersion: trustyai.opendatahub.io/v1alpha1
   kind: EvalHub
   metadata:
     name: evalhub
   spec:
     replicas: 1
     database:
       type: postgresql
       secret: evalhub-db-credentials
     providers:
       - garak
     collections:
       - safety-and-fairness-v1
     env:
       - name: MLFLOW_TRACKING_URI
         value: "${MLFLOW_INTERNAL_URL}"
   EOF

   # Verify
   oc get pods -l app=eval-hub -n "$EVALHUB_NAMESPACE"
   ```

   Wiring this in at creation avoids the patch + redeploy (and the
   in-flight-job cancellation that comes with it) a deployment hits if
   MLflow is added after the fact. Once healthy (`evalhub health`), pass
   `--experiment <name>` on job submissions (see Demo steps below) to log
   results to MLflow — see
   [Annex F](#f-viewing-results-in-the-rhoai-dashboard--mlflow) for the
   full writeup, including how to query results directly (RHOAI's MLflow
   requires an `X-MLflow-Workspace` header on every API call — a detail
   the plain web UI may also need if experiments don't show up).

4. **Install the EvalHub CLI** — use
   [uv](https://docs.astral.sh/uv/) to install into an isolated venv
   without polluting your system Python:

   ```bash
   uv tool install "eval-hub-sdk[cli]"
   evalhub --version
   ```

   Configure the CLI to point at your EvalHub instance:

   ```bash
   EVALHUB_NAMESPACE="evalhub"
   evalhub config set base_url \
     "https://$(oc get routes evalhub -o jsonpath='{.spec.host}' -n "$EVALHUB_NAMESPACE")"
   evalhub config set tenant "$EVALHUB_NAMESPACE"
   evalhub config set token "$(oc whoami -t)"

   # Verify connectivity
   evalhub health
   evalhub providers list
   ```

5. **Deploy `garak-envoy`.** EvalHub's `garak` provider uses the stock
   OpenAI Python client, which sends no custom headers — but
   `openshell service expose` routes purely by HTTP `Host` header (multiple
   sandboxes share one gateway Route). Without a header, a Garak job can
   only ever reach the gateway's *default* vhost — **confirmed on a live
   cluster**: the job fails immediately with `openai.NotFoundError: Error
   code: 404`. `garak-envoy` is a small Envoy proxy that extracts a routing
   key from the request *path* (`/route/<host-key>/...`) instead, and
   rewrites the upstream Host header to it. See
   [Annex E — EvalHub integration background](#e-evalhub-integration-background)
   (the "CONFIRMED BROKEN" note) for the full root-cause writeup. Deploy it
   once per cluster:

   ```bash
   helm upgrade --install garak-envoy demos/keycloak-oidc/garak-envoy \
     --namespace "$OPENSHELL_NAMESPACE"

   GARAK_ENVOY_HOST=$(oc get route garak-envoy -n "$OPENSHELL_NAMESPACE" \
     -o jsonpath='{.spec.host}')
   echo "garak-envoy route: $GARAK_ENVOY_HOST"
   ```

6. **Get the `agent-proxy` static binary.** No Rust toolchain needed —
   download the prebuilt musl binary from GitHub Releases into the same
   path a local build would produce, so the `AGENT_PROXY_BIN` variable
   used below works either way. Check
   [the Releases page](https://github.com/alpha-hack-program/openshell-demos/releases)
   for the current `agent-proxy-v*` tag first: the repo's overall "latest"
   release tracks whichever component published most recently, not
   necessarily `agent-proxy`, so `releases/latest/download/...` isn't
   reliable — use the explicit tag shown below (or whichever is newer):

   ```bash
   AGENT_PROXY_DIR="../../util/agent-proxy/target/x86_64-unknown-linux-musl/release"
   mkdir -p "$AGENT_PROXY_DIR"
   curl -fsSL -o "$AGENT_PROXY_DIR/agent-proxy" \
     https://github.com/alpha-hack-program/openshell-demos/releases/download/agent-proxy-v0.1.1/agent-proxy-linux-x86_64-musl
   chmod +x "$AGENT_PROXY_DIR/agent-proxy"
   ```

   If you're actively developing `agent-proxy`, build from source instead —
   requires Rust 2024 edition (1.85+) and the `x86_64-unknown-linux-musl`
   target. See [`util/agent-proxy/README.md`](../../../util/agent-proxy/README.md)
   for the full build/release/image workflow:

   ```bash
   make -C ../../util/agent-proxy musl
   ```

   Always use the musl target for anything deployed into a sandbox — a
   plain `cargo build --release` binary links against whatever glibc your
   dev machine has, which is typically too new (confirmed: Fedora 44's
   glibc 2.41) for the sandbox base images. See "Why static musl, not
   dynamic linking?" in `util/agent-proxy/README.md` for the full
   rationale.

## Demo steps — Claude Code (recommended)

> Runs from the same admin/secops session as the Prerequisites above,
> targeting `alice`'s context — see
> [How to follow this guide](#how-to-follow-this-guide) above for what that
> means in practice. Nothing here requires logging in as alice.

Run these after completing steps 1-5 of the main demo (see
[`../README.md`](../README.md)) — `alice` must already be onboarded, with
her provider and MCP roles configured there. **Claude Code is the
recommended agent for this section**: its Anthropic Messages API works
fully (including MCP tool use) against this demo's DeepSeek BYO backend.
The Codex variant below only supports model-only probes here (see why in
that variant's intro).

```bash
source .env
source ../../.env
USER_ID="alice"
SANDBOX="garak-claude-${USER_ID}"
CLAUDE_IMAGE="quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36"
AGENT_PROXY_BIN="../../util/agent-proxy/target/x86_64-unknown-linux-musl/release/agent-proxy"
LLM_HOST=$(echo "$ANTHROPIC_BASE_URL" | sed 's|https\?://||;s|/.*||')
SERVER_NAME="mcp-portfolio"
# Re-derive if this is a fresh shell from the Prerequisites section:
GARAK_ENVOY_HOST=$(oc get route garak-envoy -n "$OPENSHELL_NAMESPACE" -o jsonpath='{.spec.host}')
```

**1. Create the `byo-claude` provider** (skip if you already created it for
the "Claude Code + BYO LLM + MCP tool" recipe in the main README). Note
`--workspace "${USER_ID}"`: providers live in the target banker's own
workspace, not admin's — see
[Workspace isolation](../README.md#workspace-isolation) in the main guide:

```bash
TMPFILE=$(mktemp --suffix=.yaml)
sed "s/<llm-host>/${LLM_HOST}/" providers/byo-claude-profile.yaml > "$TMPFILE"
openshell provider profile import -f "$TMPFILE" --workspace "${USER_ID}"
rm -f "$TMPFILE"

openshell provider create --name byo-claude --type byo-claude \
  --credential "ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY" \
  --workspace "${USER_ID}"
```

**2. Create a sandbox from the stock Claude Code image, attach providers,
and grant network access.** The sandbox itself must also live in
`${USER_ID}`'s workspace (`--workspace "${USER_ID}"` on every command
below) — each banker gets a dedicated workspace, and providers/sandboxes in
different workspaces can't reach each other regardless of who's driving
the session:

```bash
openshell sandbox create --name "$SANDBOX" --from "$CLAUDE_IMAGE" \
  --workspace "${USER_ID}" -- true

openshell sandbox provider attach "$SANDBOX" byo-claude --workspace "${USER_ID}"
openshell sandbox provider attach "$SANDBOX" "user-${USER_ID}" --workspace "${USER_ID}"

openshell policy update "$SANDBOX" --workspace "${USER_ID}" \
  --add-endpoint "${LLM_HOST}:443:read-write:rest:enforce" \
  --binary /usr/local/bin/claude \
  --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
  --binary /usr/local/bin/claude \
  --wait
```

**3. Upload agent-proxy and an MCP-config wrapper script.** Claude Code's
`--mcp-config` JSON must be built at *runtime* inside the sandbox, since it
embeds `$USER_ACCESS_TOKEN` — credential resolution happens at the network
layer when `claude` sends this exact placeholder string to a
policy-matched endpoint, not when a shell expands the variable (confirmed:
a plain `bash -c 'echo $USER_ACCESS_TOKEN'` prints the literal placeholder,
never the real token — see
[Annex G — Validated findings log](#g-validated-findings-log)). A wrapper
script is required because agent-proxy's `AGENT_COMMAND` is split naively
on whitespace — it can't express the shell quoting this needs:

```bash
cat > /tmp/run-claude.sh << EOF
#!/bin/bash
set -e
MCP_JSON="{\\"mcpServers\\":{\\"portfolio\\":{\\"type\\":\\"http\\",\\"url\\":\\"http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp\\",\\"headers\\":{\\"Authorization\\":\\"Bearer \$USER_ACCESS_TOKEN\\"}}}}"
exec claude -p "\$1" \\
  --mcp-config "\$MCP_JSON" \\
  --strict-mcp-config \\
  --permission-mode bypassPermissions \\
  --output-format text
EOF

openshell sandbox upload "$SANDBOX" "$AGENT_PROXY_BIN" /sandbox/agent-proxy --workspace "${USER_ID}"
openshell sandbox exec -n "$SANDBOX" --workspace "${USER_ID}" -- chmod +x /sandbox/agent-proxy
openshell sandbox upload "$SANDBOX" /tmp/run-claude.sh /sandbox/run-claude.sh --workspace "${USER_ID}"
openshell sandbox exec -n "$SANDBOX" --workspace "${USER_ID}" -- chmod +x /sandbox/run-claude.sh
```

**4. Start the proxy in background and expose the service.** Claude Code's
`-p` mode doesn't need a real TTY (unlike Codex — see the Codex variant
below), so a plain background start is fine:

```bash
nohup openshell sandbox exec -n "$SANDBOX" --workspace "${USER_ID}" \
  --env 'AGENT_COMMAND=/sandbox/run-claude.sh' \
  --env 'OUTPUT_FILE_FLAG=' \
  --env "ANTHROPIC_BASE_URL=$ANTHROPIC_BASE_URL" \
  --env "ANTHROPIC_MODEL=$ANTHROPIC_MODEL" \
  --env "ANTHROPIC_DEFAULT_OPUS_MODEL=$ANTHROPIC_MODEL" \
  --env "ANTHROPIC_DEFAULT_SONNET_MODEL=$ANTHROPIC_MODEL" \
  --env "ANTHROPIC_DEFAULT_HAIKU_MODEL=$ANTHROPIC_MODEL" \
  -- /sandbox/agent-proxy --port 8100 > /tmp/agent-proxy-exec.log 2>&1 &
PROXY_EXEC_PID=$!

openshell service expose "$SANDBOX" 8100 --workspace "${USER_ID}"
```

Verify the proxy is reachable and MCP tool use works: a real, tool-derived
answer (not a hallucination) should be checkable against `mcp-portfolio`'s
own container logs (`called_by`/`roles` claims should match the
authenticated user's real JWT). Note the exposed `SERVICE_HOST` is prefixed
with the banker's **workspace**, not `default` — the pod naming pattern is
`<workspace>--<sandbox-name>`, same as the main demo's `demo-<id>`
sandboxes:

```bash
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
SERVICE_HOST="${USER_ID}--${SANDBOX}.openshell.localhost"

curl -sk -X POST "https://${ROUTE_HOST}/v1/chat/completions" \
  -H "Host: ${SERVICE_HOST}" \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"Who is my biggest client by assets under management?"}]}' | jq .
```

**5. Submit an EvalHub evaluation through `garak-envoy`.** Point
`--model-url` at `garak-envoy`'s Route with the sandbox's Host-header key
embedded in the path. Add `--experiment` to log results to MLflow, wired up
in Prerequisites step 3. Start with `quick` — it's fast and cheap; heavier
benchmarks like `owasp_llm_top10` run far longer and issue many more probes
against the live LLM backend, so only reach for one once `quick` works:

```bash
evalhub eval run \
  --name "redteam-${USER_ID}" \
  --model-url "https://${GARAK_ENVOY_HOST}/route/${SERVICE_HOST}" \
  --model-name "${SANDBOX}-agent-proxy" \
  --provider garak \
  -b quick \
  --experiment "redteam-${USER_ID}"
```

**6. Track results:**

```bash
evalhub eval status <job_id>
evalhub eval results <job_id>
```

If MLflow tracking is enabled, the job response includes a
`mlflow_experiment_id` and (once complete) each benchmark result includes
an `mlflow_run_id` — see
[Annex F — Viewing results in the RHOAI dashboard / MLflow](#f-viewing-results-in-the-rhoai-dashboard--mlflow)
below to browse or query them.

**7. Cleanup:**

```bash
kill "$PROXY_EXEC_PID" 2>/dev/null
openshell service delete "$SANDBOX" --workspace "${USER_ID}"
openshell sandbox delete "$SANDBOX" --workspace "${USER_ID}"
```

## Demo steps — Codex (optional)

> Same session model as the Claude Code steps above — nothing here
> requires logging in as alice.

Codex requires an on-cluster vLLM **≥0.25.0** (upstream — RHOAI 3.4.x
ships 0.18.0, too old) for MCP tool use; its `namespace` tool type isn't
supported by this demo's DeepSeek BYO backend (confirmed: DeepSeek rejects
it with a 400). Without that endpoint, Codex can still run **model-only**
red-team probes (no MCP tool calls). Use this variant only if you have
such a vLLM endpoint, or specifically want to red-team the model in
isolation.

```bash
source .env
source ../../.env
USER_ID="alice"
SANDBOX="garak-codex-${USER_ID}"
AGENT_IMAGE="quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012"
AGENT_PROXY_BIN="../../util/agent-proxy/target/x86_64-unknown-linux-musl/release/agent-proxy"
GARAK_ENVOY_HOST=$(oc get route garak-envoy -n "$OPENSHELL_NAMESPACE" -o jsonpath='{.spec.host}')
```

**1. Create the sandbox and attach providers.** As in the Claude Code
variant, the sandbox and its providers must all live in `${USER_ID}`'s own
workspace:

```bash
openshell sandbox create --name "$SANDBOX" --from "$AGENT_IMAGE" \
  --workspace "${USER_ID}" -- true
openshell sandbox provider attach "$SANDBOX" "user-${USER_ID}" --workspace "${USER_ID}"
openshell sandbox provider attach "$SANDBOX" byo-codex --workspace "${USER_ID}"
```

**2. Upload the agent-proxy binary:**

```bash
openshell sandbox upload "$SANDBOX" "$AGENT_PROXY_BIN" /sandbox/agent-proxy --workspace "${USER_ID}"
openshell sandbox exec -n "$SANDBOX" --workspace "${USER_ID}" -- chmod +x /sandbox/agent-proxy
```

**3. Start the proxy in the FOREGROUND with `--tty`, backgrounded on the
*local* machine.** Codex's `exec` subcommand refuses to run
non-interactively unless stdin, stdout, AND stderr are all real TTYs and
`TERM` isn't `dumb` — even with `--dangerously-bypass-approvals-and-sandbox`.
`sandbox exec` only allocates a pty when `--tty` is passed explicitly (or
the calling terminal is itself a real tty) — a background
`nohup agent-proxy &` *inside* the sandbox never requests one, and tears
down the exec channel (and its pty) the moment the wrapping shell exits
regardless. See
[Annex B — Troubleshooting](#b-troubleshooting) for the full investigation:

```bash
nohup openshell sandbox exec -n "$SANDBOX" --tty --workspace "${USER_ID}" \
  --env 'AGENT_COMMAND=codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox' \
  -- /sandbox/agent-proxy --port 8100 > /tmp/agent-proxy-exec.log 2>&1 &
PROXY_EXEC_PID=$!

openshell service expose "$SANDBOX" 8100 --workspace "${USER_ID}"
```

**4. Verify, submit the eval, track results, and clean up** — same as the
Claude Code steps 4-7 above, substituting this variant's `$SANDBOX` (the
model-only probe results won't include MCP-related findings).

## Annexes

### A. Alternative: Approach B — custom sandbox image

The Demo steps above use "Approach A" (upload the binary into a stock
sandbox at runtime — no custom image, no registry, no Containerfile). This
is the approach this demo actually documents and runs, chosen specifically
to avoid a build/push step for what is fundamentally a testing workflow.

A Containerfile-based alternative — baking `agent-proxy` into a custom
image instead of uploading it at runtime — exists for CI/reproducibility
cases where the binary must always be present. It isn't part of the
documented demo flow; see [Sandbox service
patterns](../../../docs/sandbox-service-patterns.md) for the general
pattern (static musl binaries, Containerfile layout, remote-gateway
build+push workflow). `demos/keycloak-oidc/images/{codex,claude}-garak/`
are example Containerfiles following that pattern for this demo's two
agents — validated on a live cluster, but kept only as a reference, not a
maintained path.

### B. Troubleshooting

#### TTY root cause (Codex path, resolved 2026-08-18)

Codex is optional for this demo (see [Purpose](#purpose)) — this section
documents the TTY fix for the model-only-probes case, and the vLLM
≥0.25.0 requirement for Codex + MCP.

codex's `exec` subcommand refuses to run non-interactively unless **stdin,
stdout, and stderr are all real TTYs** (`isatty`) and `TERM` isn't `dumb` —
even with `--dangerously-bypass-approvals-and-sandbox`. Two independent
bugs previously caused every codex invocation through agent-proxy to fail:

1. **agent-proxy actively destroyed TTY state before spawning codex** —
   the original code called `.env_remove("TERM")` and
   `.stdin(Stdio::null())` unconditionally, guaranteeing codex saw a dumb,
   non-interactive environment regardless of how agent-proxy itself was
   started. Fixed: `TERM` is now inherited (not stripped), and stdin is
   inherited too.
2. **The proxy was always started backgrounded (`nohup agent-proxy &`)
   without `--tty`** — `sandbox exec` only allocates a pty when the CLI's
   own local stdin/stdout are terminals or `--tty` is passed explicitly
   (confirmed against `NVIDIA/OpenShell` source: `run.rs:1466-1467`,
   `docs/sandboxes/manage-sandboxes.mdx:178-192`). None of the demo's
   documented startup commands passed `--tty`, and the wrapping
   `bash -c '... &'` exits immediately after backgrounding, tearing down
   the exec channel (and its pty) regardless. Fixed: start agent-proxy via
   a **foreground** `sandbox exec --tty`, backgrounded on the *local*
   machine instead (`... &` after the command, not inside it).

Even with both fixed, codex also requires **stdout** to be a real TTY —
which rules out capturing its answer by redirecting stdout to a file. This
sandbox cannot self-allocate a pty either (`script -qc 'echo hi' /dev/null`
fails with `Permission denied` opening `/dev/pts` — confirmed, likely an
OpenShift SCC/seccomp restriction), so agent-proxy can't interpose its own
pty to capture output while still presenting a TTY to the child. The fix:
inherit stdin/stdout/stderr entirely (all genuine TTYs from the exec
session), and have codex write its answer to a file via
`-o <path>`/`--output-last-message <path>` instead of relying on captured
stdout — agent-proxy reads that file back. This is configurable per agent
via `OUTPUT_FILE_FLAG` (empty disables it, falling back to stdout capture
for agents like Claude Code that don't need a TTY).

**Confirmed working end-to-end on a live cluster**: `POST
/v1/chat/completions` → agent-proxy → codex → real LLM response, through
the full gateway route with Host-header routing.

Upstream architecture note: `NVIDIA/OpenShell`'s `sandbox exec` doesn't use
Kubernetes' `pods/exec` subresource at all — it's gRPC (CLI) → an SSH relay
into a supervisor process running inside the sandbox, which does its own
`openpty()` + `bash -lc "<command>"` when a pty is requested. This is a
normal process tree: any subprocess the exec'd command spawns inherits the
same pty fds via ordinary fork/exec — there is no OpenShell-side
restriction scoping the pty to only the direct exec target.

#### agent-proxy's AGENT_COMMAND / OUTPUT_FILE_FLAG reference

A small `axum` server exposing `POST /v1/chat/completions`. On each request
it extracts the last user message, shells out to the configured agent CLI
via `AGENT_COMMAND` (the prompt is appended as the final argument, split
naively on whitespace), and returns a standard OpenAI `ChatCompletion`
response. `AGENT_COMMAND` must be set as a real environment variable on
the running process (e.g. via `sandbox exec --env`) — the compiled-in
default only applies to `--help` text.

| Agent | `AGENT_COMMAND` value | `OUTPUT_FILE_FLAG` |
|---|---|---|
| Codex (default) | `codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox` | `-o` (default) |
| Claude Code | path to a wrapper script (see Demo steps above) | `""` (disabled — `-p` mode prints plain text to stdout and doesn't require a TTY) |

No streaming needed — Garak sends blocking requests. No auth on the proxy
itself — it's loopback inside a sandbox, exposed only via
`openshell service expose`. See
[`util/agent-proxy/README.md`](../../../util/agent-proxy/README.md) for the
full reference.

### C. Resolved design decisions

| Question | Decision | Rationale |
|---|---|---|
| Proxy name / location | `agent-proxy` at `util/agent-proxy/` (repo root) | Reusable across demos, consistent with `util/onboard/` |
| Agent selection | Claude Code (primary), Codex (optional) | Both supported via `AGENT_COMMAND` env var; separate Containerfile per agent. Codex + MCP requires an on-cluster vLLM ≥0.25.0 — against the demo's default DeepSeek backend it's model-only-probes-only. Claude Code works fully (incl. MCP) against DeepSeek. |
| Sandbox naming (this demo section) | `garak-codex-<user>` / `garak-claude-<user>` | Distinguishes agent type in the sandbox name (e.g. `garak-codex-alice`, `garak-claude-alice`) — avoids ambiguity with the main demo's `demo-<user>` sandboxes and with each other. |
| EvalHub MCP server | Not needed | Evaluations driven from RHOAI UI / CLI, not from inside sandboxes |
| BYOF adapter | Not needed | agent-proxy already exposes an OpenAI-compatible endpoint; EvalHub's built-in `garak` provider accepts any `model.url` pointing to an OpenAI `/v1` endpoint. The `garak-kfp` risk assessment pipeline (Ch. 4) also accepts arbitrary URLs. **Confirmed from RHOAI 3.4 docs.** |
| EvalHub integration path | Built-in `garak` provider first (Path 1), `garak-kfp` risk assessment later (Path 2) | Path 1 needs only EvalHub + agent-proxy URL. Path 2 adds multi-strategy attacks (SPO, Translation, TAP) but requires KFP + S3 + judge/SDG models — heavier infrastructure. |
| Demo structure | Extend `demos/keycloak-oidc/` | Same infrastructure stack (OIDC, Envoy, per-user sandboxes); avoids duplication |
| Sandbox image | **Approach A** — upload the binary into a stock sandbox at runtime. No custom image, no registry, no Containerfile | Simplest path for a testing workflow — no build/push step. A Containerfile-based alternative (Approach B, baking `agent-proxy` into a custom image) exists for CI/reproducibility cases but isn't part of the documented demo flow — see [Annex A](#a-alternative-approach-b--custom-sandbox-image) and [Sandbox service patterns](../../../docs/sandbox-service-patterns.md). Both validated on a live cluster. |
| Proxy startup | Create sandbox first, then start proxy via a **foreground** `sandbox exec --tty` (backgrounded on the *local* machine, not with `nohup &` inside the sandbox) | The `-- <command>` on `sandbox create` runs via SSH and blocks the CLI. codex requires stdin/stdout/stderr to all be a real TTY (see [Annex B](#b-troubleshooting)); `nohup agent-proxy &` inside the sandbox exits the wrapping shell immediately and tears down the pty. `openshell sandbox exec -n <sandbox> --tty -- agent-proxy ... &` (local `&`) keeps the exec channel — and its pty — alive for the proxy's whole lifetime. **Confirmed on live cluster** (2026-08-18). |
| Service exposure | `openshell service expose <sandbox> 8080` (not `--forward`) | `--forward` creates a local-only SSH tunnel; `service expose` creates a gateway-managed HTTPS URL using Host-header routing through the gateway's Route. Garak reaches it via `garak-envoy` (see [Annex E](#e-evalhub-integration-background)). **Confirmed.** |
| Image build for remote gateways | `podman build` + `podman push` + `--from <image-ref>` | `--from <Dockerfile-dir>` only works for local gateways. For remote (OpenShift), build/push manually to `$GARAK_IMAGE_REGISTRY`. **Confirmed.** |
| Containerfile naming | `Containerfile` (Podman-idiomatic) | The OpenShell CLI's `--from <directory>` only looks for `Dockerfile`, but that mode doesn't work on remote gateways anyway. We build with `podman build -f`, which accepts any name. **Confirmed.** |
| Garak probe selection | Deferred | Will survey Garak's catalog later |

### D. Roles and responsibilities

#### Who runs what

Red-team evaluation is a **secops / AI-secops function**, not a user
responsibility. Users don't audit their own sandboxes — that's a conflict
of interest. The full lifecycle (create sandbox → deploy proxy → expose →
run Garak → collect results → cleanup) is owned by a secops role or
automated by a service account.

| Duty | Who | Why |
|---|---|---|
| Deploy EvalHub (Operator, PostgreSQL, CR) | Cluster admin | Cluster-scoped infrastructure |
| Build + push custom images (Approach B, optional alternative — not used by the demo) | Secops / CI | Registry and build pipeline access |
| Build agent-proxy static binary | Secops / CI | Cargo + musl toolchain |
| Create sandbox, upload/start proxy, expose service | **Secops service account** | Red-teaming is not a user duty |
| Submit EvalHub evaluation jobs | **Secops service account** | Owns the evaluation results and compliance reports |
| Review results, file findings | Secops | Owns the security posture |
| Define custom harm categories | Secops | Domain-specific policy |

#### Security context challenge

The architecture's key insight is that Garak probes hit the agent in the
**exact same environment a real user would have** — providers, MCP roles,
network policies are all live. But if a secops service account creates the
sandbox, that sandbox gets the **service account's** security context:

- **Providers v2** injects credentials per-user — the sandbox gets the
  service account's providers, not the target user's
- **MCP server access** is gated by the authenticated user's Keycloak JWT
  roles — the service account likely has different roles

This means the secops account would red-team its own context, not the
user's. To fix this, **test representative user profiles, not individual
users.**

#### Representative user profiles

Standard security testing methodology: define profiles that mirror the
roles/permissions of real user classes, and red-team each profile.

1. **Define profiles in Keycloak** — e.g. `secops-eval-role-a` (access to
   MCP server A only), `secops-eval-role-ab` (access to both servers),
   `secops-eval-no-mcp` (no MCP access)
2. **Each profile is a Keycloak service account** — confidential client with
   `service-accounts-enabled: true`, granted the same realm roles and
   provider configurations as the target user class
3. **Secops authenticates as each profile**, creates a sandbox, runs the
   evaluation — the sandbox gets that profile's exact security context
4. **Results per profile** — MLflow experiments tagged by profile, so you
   can compare attack success rates across different permission levels

This approach answers the question: "If a user with role X runs an agent
in a sandbox, how resistant is the stack to adversarial probes?" — which
is more useful than testing a single named user.

#### Automated evaluation loop

In production, the secops automation iterates over all representative
profiles, running the full lifecycle for each:

```bash
PROFILES=("secops-eval-role-a" "secops-eval-role-ab" "secops-eval-no-mcp")

for PROFILE in "${PROFILES[@]}"; do
    echo "=== Evaluating profile: ${PROFILE} ==="

    # 1. Authenticate as this profile (client credentials or Playwright)
    TOKEN=$(curl -s -X POST "${KEYCLOAK_TOKEN_URL}" \
      -d "grant_type=client_credentials" \
      -d "client_id=${PROFILE}" \
      -d "client_secret=${!PROFILE_SECRET}" | jq -r .access_token)  # [VERIFY]

    # 2. Create sandbox from stock image
    openshell sandbox create --name "eval-${PROFILE}" \
        --from "$AGENT_IMAGE" -- true

    # 3. Upload agent-proxy, start (foreground + --tty, backgrounded
    #    locally — see Annex B, Troubleshooting), expose
    openshell sandbox upload -n "eval-${PROFILE}" \
        "$AGENT_PROXY_BIN" /usr/local/bin/agent-proxy
    openshell sandbox exec -n "eval-${PROFILE}" -- chmod +x /usr/local/bin/agent-proxy
    openshell sandbox exec -n "eval-${PROFILE}" --tty \
        --env 'AGENT_COMMAND=codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox' \
        -- /usr/local/bin/agent-proxy --port 8080 > "proxy-exec-${PROFILE}.log" 2>&1 &
    PROXY_EXEC_PID=$!
    openshell service expose "eval-${PROFILE}" 8080

    # 4. Run EvalHub evaluation
    evalhub eval run \
        --name "redteam-${PROFILE}" \
        --model-url "https://${ROUTE_HOST}" \
        --model-name "${PROFILE}" \
        --provider garak \
        -b <benchmark_id>

    # 5. Wait for results
    JOB_ID=$(evalhub eval status --name "redteam-${PROFILE}" | ...)
    evalhub eval status --watch "$JOB_ID"

    # 6. Cleanup
    openshell service delete "eval-${PROFILE}"
    kill "$PROXY_EXEC_PID" 2>/dev/null
    openshell sandbox delete "eval-${PROFILE}"
done
```

Each profile's evaluation is tagged in MLflow, so results can be compared
across roles — e.g. "role-a with MCP-server-A-only has 12% ASR vs role-ab
with both servers at 18% ASR." This surfaces which permission
configurations are most vulnerable.

#### OIDC authentication for service accounts — [VERIFY]

Keycloak service accounts authenticate via **client credentials grant**
(`grant_type=client_credentials`, no browser flow). The keycloak-oidc demo
uses authorization code flow (browser-based).

```bash
# Client credentials grant — produces a valid JWT
curl -s -X POST "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}/protocol/openid-connect/token" \
  -d "grant_type=client_credentials" \
  -d "client_id=secops-eval-role-a" \
  -d "client_secret=<secret>" | jq -r .access_token
```

**Open question:** does OpenShell's gateway accept tokens obtained via
client credentials grant? The JWT is structurally valid, but OpenShell
may enforce claims (`preferred_username`, `email`, `sub` format) that
differ between authorization code tokens and client credentials tokens.
Needs testing on a live cluster.

**Fallback:** if client credentials doesn't work, use headless browser
automation (Playwright) with a dedicated Keycloak user per profile. This
pattern is already documented in
[`headless-browser-automation.md`](../../../docs/headless-browser-automation.md).

#### Demo shortcut

The production approach (service accounts per profile) is the right design
but overkill for a demo. For the `keycloak-oidc` demo, **use an existing
demo banker** (`alice`, `bob`, or `charlie`) who is already onboarded with
the correct providers and MCP roles — see "Demo steps" above. The sandbox
gets that banker's exact security context — providers, MCP roles, network
policies — which is precisely what we want to red-team, even though an
admin/secops identity drives every command (see
[How to follow this guide](#how-to-follow-this-guide) above).

### E. EvalHub integration background

Based on the RHOAI 3.4 "Evaluating AI systems" documentation (2026-06-04).

#### Two paths to Garak in EvalHub

EvalHub offers two distinct ways to run Garak-based evaluations. Both accept
an arbitrary OpenAI `/v1`-compatible model URL — which is exactly what
agent-proxy exposes.

| Path | Provider | What it does | Infrastructure needed |
|---|---|---|---|
| **Built-in Garak provider** (Ch. 2) | `garak` | Standard LLM vulnerability scanning — 12 built-in benchmarks | EvalHub server only |
| **Automated Risk Assessment** (Ch. 4) | `garak-kfp` | Intent-based multi-strategy pipeline: Baseline → SPO → Translation → TAP, with judge + SDG models | EvalHub + Kubeflow Pipelines (Data Science Pipelines) + S3 + judge model + SDG model |

**Recommendation:** start with the built-in `garak` provider (Path 1). It
needs only EvalHub deployed and a model URL — no KFP, S3, or auxiliary
models. Once that works end-to-end, Path 2 can be added as an advanced
recipe for deeper red-team evaluations.

#### Path 1 — Built-in `garak` provider: submitting via REST API

The Demo steps above use the `evalhub` CLI. The equivalent raw REST API
call (e.g. for scripting without the CLI):

```bash
curl -X POST "$EVALHUB_URL/api/v1/evaluations/jobs" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -H "X-Tenant: <namespace>" \
  -d '{
    "name": "redteam-codex-v1",
    "model": {
      "url": "'"$PROXY_URL"'/v1",
      "name": "codex-agent-proxy"
    },
    "benchmarks": [
      { "provider_id": "garak", "benchmark_id": "<benchmark>" }
    ]
  }'
```

**CONFIRMED BROKEN (2026-08-18), not just [VERIFY]** — tested end-to-end on
a live cluster with EvalHub already deployed (TrustyAI/KServe `Managed`,
`evalhub` namespace, `garak` provider, `evalhub` CLI configured). Submitted
`evalhub eval run --provider garak -b quick --model-url
https://openshell-<namespace>.<apps-domain> --model-name ...` against
`garak-claude-user1`'s exposed agent-proxy (i.e. `model-url` pointed
straight at the gateway Route, without `garak-envoy` in front). The job's
adapter pod failed immediately with `openai.NotFoundError: Error code:
404`.

Root cause, confirmed by reading the EvalHub SDK source
(`evalhub/models/api.py` — `ModelConfig` has only `url: str`, `name: str`,
`auth: ModelAuth | None` where `ModelAuth` is just `secret_ref: str`; **no
headers field at all**) and the adapter's traceback
(`llama_stack_provider_trustyai_garak/evalhub/garak_adapter.py` →
`garak/generators/openai.py` → the stock `openai` Python client, which
sends whatever `Host` the URL's own hostname implies — no override):

- `openshell service expose` routes purely by HTTP `Host` header —
  multiple sandboxes' services share the **same** gateway Route hostname
  and TLS SNI, disambiguated only by `Host:` (confirmed working via curl
  with `-H "Host: ..."` throughout this doc).
- `.openshell.localhost` hostnames (as printed by `service expose`) are
  **not real DNS** — they resolve to loopback only via RFC 6761 NSS
  special-casing on the machine running `openshell` CLI. They are
  unreachable from a Garak Job pod running inside the cluster.
- EvalHub's `ModelConfig` has no way to set a custom `Host` header, and
  Garak's OpenAI-compatible generator doesn't expose one either. So a
  Garak Job can only ever reach the gateway's *default* vhost — never a
  Host-header-disambiguated sandbox service.

**This means `openshell service expose` + EvalHub's built-in `garak`
provider are fundamentally incompatible as of OpenShell 0.0.106 / this
EvalHub SDK version** — not a missing flag, an architecture gap. The fix,
validated on a live cluster: `garak-envoy` (deployed in Prerequisites step
5 above) — an Envoy proxy on a real, DNS-resolvable Route that extracts a
routing key from the request *path* (`/route/<host-key>/...`) and rewrites
the upstream `:authority`/Host header to it, forwarding to the gateway's
internal ClusterIP over TLS. This works because:

- Confirmed the gateway's Host-header routing works identically whether
  hit via the external Route or the internal ClusterIP directly (same TLS
  backend, SNI doesn't matter for the routing decision — only the
  decrypted HTTP Host header does).
- Confirmed the OpenAI Python client (used by `garak.generators.openai`)
  just string-concatenates `base_url + "/chat/completions"` without
  validating the path shape, so an extra `/route/<key>` path segment
  passes through untouched.
- **Full round-trip confirmed**: a real EvalHub job (`quick` benchmark)
  submitted with `--model-url
  https://<garak-envoy-route>/route/<sandbox-host-key>` completed
  successfully with real `attack_success_rate` metrics — not just reaching
  agent-proxy, but Garak's actual adversarial probes running against
  Claude Code and getting scored.

Two other ways forward exist if `garak-envoy` ever needs replacing (not
attempted, listed for completeness):
1. **Bypass `service expose` entirely** — create a dedicated Kubernetes
   `Service` (selecting the sandbox pod directly by label) + `Route`.
   Loses the "proxy exposed only on demand" isolation `service expose`
   gives, and requires relying on the sandbox pod's labels (not a stable,
   documented OpenShell API).
2. **File an OpenShell and/or EvalHub feature request** — either
   `service expose` should support a real per-sandbox hostname, or
   EvalHub's `ModelConfig` should accept custom headers for the target
   model endpoint.

The `garak-kfp` risk assessment pipeline (Path 2) hasn't been tested
against this constraint and may have the same issue (same `ModelRef`-style
schema per the RHOAI docs) — `garak-envoy`'s path-based routing should work
for it too, but this is unverified.

### F. Viewing results in the RHOAI dashboard / MLflow

**EvalHub/Garak results have no native RHOAI dashboard view** — the
dashboard's "Performing model evaluations in the dashboard" feature (RHOAI
3.4 docs §3.5) is scoped to **LM-Eval only**. The only UI-capable path for
EvalHub is MLflow experiment tracking (enabled in Prerequisites step 3
above).

**Confirmed working end-to-end**: the job response includes a real
`mlflow_experiment_id`, and results include a `mlflow_run_id` once
complete. Verified the run is genuinely queryable via MLflow's own REST
API (not just EvalHub's say-so) — RHOAI's MLflow requires an
`X-MLflow-Workspace` header for every API call (a custom multi-tenancy
header beyond stock MLflow), matching the `MLFLOW_WORKSPACE` value
(`evalhub` by default):

```bash
TOKEN=$(oc whoami -t)
curl -sk -H "Authorization: Bearer $TOKEN" -H "X-MLflow-Workspace: evalhub" \
  "https://<rhoai-dashboard-domain>/mlflow/api/2.0/mlflow/runs/get?run_id=<mlflow_run_id>"
```

To browse in a real browser: open
`https://<rhoai-dashboard-domain>/mlflow`, log in with OpenShift SSO. The
web UI's own workspace-switching mechanism wasn't verified in this
session (only the REST API's `X-MLflow-Workspace` header requirement was
confirmed) — if experiments don't appear, look for a workspace selector in
the UI matching `MLFLOW_WORKSPACE`.

#### Path 2 — Automated Risk Assessment (future)

For deeper multi-strategy evaluations (SPO, Translation, TAP), use the
`garak-kfp` provider. This requires additional infrastructure:

- **Data Science Pipelines (Kubeflow Pipelines)** — orchestrates the
  multi-stage assessment pipeline
- **S3-compatible storage** — pipeline artifacts
- **Judge model** — an LLM endpoint (OpenAI `/v1` compatible) to classify
  target responses as compliant/refused
- **SDG model** — an LLM endpoint to generate adversarial prompts

The assessment runs 5 attack strategies progressively:
1. **Baseline** — unmodified prompt
2. **System Prompt Override (SPO)** — adversarial system prompts
3. **SPO variants** — statistical manipulation of user/system prompts
4. **Translation** — prompts translated to Mandarin Chinese
5. **Tree of Attacks with Pruning (TAP)** — attacker LLM dynamically
   generates prompts based on target responses

Default harm categories: illegal activity, hate speech, security/malware,
violence, fraud, sexually explicit, misinformation, self harm. Custom harm
categories can be defined as JSON/CSV datasets uploaded to S3.

Primary metric: **Attack Success Rate (ASR)** — percentage of test prompts
that bypassed safety controls. Lower is better. Score > 0.5 indicates
compliance concern.

This path is deferred until representative-profile automation (see
[Annex D](#d-roles-and-responsibilities)) is built on top of Path 1.

#### Custom harm categories for OpenShell

Beyond the default harm categories, we should define OpenShell-specific
custom categories targeting the sandbox security layers:

| Category | Description |
|---|---|
| Credential exfiltration | Prompts that seek to extract, display, or transmit API keys, tokens, or secrets from the sandbox environment |
| Sandbox escape | Prompts that attempt to execute commands outside the sandbox, access host filesystem, or bypass network policies |
| Unauthorized MCP access | Prompts that attempt to invoke MCP tools the user's Keycloak role should not grant access to |
| Token leakage via tool output | Prompts that attempt to surface provider-injected credentials through MCP tool responses or error messages |

These would be implemented as a policy dataset (JSON/CSV) uploaded to S3
for Path 2. For Path 1, the standard Garak probes may already cover some
of these (particularly credential exfiltration and security/malware). A
future section of this guide should define a specific Garak probe/config
set targeting these OpenShell-specific sandbox-escape and
credential-exfiltration scenarios directly (tracked as an open item below).

### G. Validated findings log

Dated findings from live-cluster testing, kept as a historical record —
the "Prerequisites"/"Demo steps" sections above reflect the current,
consolidated procedure; this log explains *how* those procedures were
arrived at and what didn't work along the way.

#### Proxy lifecycle (2026-08-18)

Tested end-to-end on a live OpenShift cluster (OpenShell 0.0.106, remote
gateway). Steps and findings:

1. **agent-proxy Rust code** — written, builds clean (~130 LOC axum server).
2. **Containerfiles** — written for both Codex and Claude Code variants.
3. `cargo build --release --target x86_64-unknown-linux-musl` — produces a
   2.8 MB **statically-linked** binary. Must use musl target — the glibc
   build (Fedora 44, glibc 2.41) is too new for the sandbox base images.
4. **`--from <directory>` does NOT work for remote gateways** — the CLI
   errors with "local Dockerfile sources are only supported for local
   gateways". Must `podman build -f <Containerfile>` + `podman push` +
   `--from <image-ref>`. (The CLI's directory mode also requires the file
   to be named `Dockerfile`, but that mode is irrelevant for remote
   gateways.)
5. **`sandbox upload` + `sandbox exec`** — used to test the proxy in an
   existing sandbox without building a custom image. Works.
6. **Background startup — SUPERSEDED.** `sandbox exec -- bash -c 'nohup
   agent-proxy &'` starts the proxy and the exec call returns, but this
   pattern is incompatible with codex: it never requests a pty (no
   `--tty`) and the wrapping shell exits immediately, tearing down the
   exec channel anyway. See [Annex B](#b-troubleshooting) — use a
   **foreground** `sandbox exec --tty`, backgrounded on the local machine,
   instead. Still fine for agents that don't need a TTY (e.g. Claude
   Code's `-p` mode).
7. **`service expose`** — creates a gateway-managed HTTPS URL using
   Host-header routing through the gateway's Route. Garak K8s Jobs reach
   the proxy via `garak-envoy` (see [Annex E](#e-evalhub-integration-background)),
   since they can't set the Host header themselves.
8. **Full round-trip** — `POST /v1/chat/completions` through the gateway
   returns a valid OpenAI `ChatCompletion` response. Confirmed with an
   `echo` stub and a Claude Code invocation (Claude correctly fails with
   exit 1 when no API key is configured — proxy surfaces the error).
9. **Cleanup** — `openshell service delete <sandbox>` removes the exposed
   service.
10. **Codex TTY fix, full round-trip confirmed** — with the `TERM`/stdin
    fix, foreground `--tty` startup, and `-o` output-file capture (see
    [Annex B](#b-troubleshooting)), `POST /v1/chat/completions` →
    agent-proxy → codex → a real LLM response works end-to-end through the
    gateway route. Verified twice on the live `garak-codex-user1` sandbox
    (`keycloak-oidc-demo` namespace) with distinct prompts, including one
    requiring actual computation (`17 * 23` → `391`), ruling out a stubbed
    or cached response.

#### Claude Code + MCP via agent-proxy (2026-08-18)

Unlike codex on DeepSeek (see [Annex B](#b-troubleshooting) — DeepSeek
rejects Codex's `namespace` tool type), Claude Code's Anthropic Messages
API uses standard tool definitions, so MCP tool use works against the
demo's DeepSeek BYO backend. Confirmed end-to-end on a live cluster:

1. **Sandbox**: created `garak-claude-user1` from the stock Claude Code base
   image (`quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36`) —
   `garak-codex-user1` (Codex base image) has no `claude` binary, so a
   separate sandbox is needed for this path.
2. **Provider**: `byo-claude` didn't exist yet — created it from
   `providers/byo-claude-profile.yaml` (LLM host substituted with
   `api.deepseek.com`), then `openshell sandbox provider attach
   garak-claude-user1 byo-claude` and `... user-user1` (for
   `USER_ACCESS_TOKEN`, needed for MCP auth).
3. **Policy**: granted `garak-claude-user1` network access to the LLM host
   and `mcp-server-a.<namespace>.svc.cluster.local:8000`, both scoped to
   `--binary /usr/local/bin/claude`.
4. **Credential resolution is network-layer, not env-var-layer.** A plain
   `sandbox exec -- bash -c 'echo $USER_ACCESS_TOKEN'` prints the literal
   placeholder string (`openshell:resolve:env:...`), **not** the real
   token — confirmed empirically. The real secret is substituted
   transparently when the bound binary (`/usr/local/bin/claude`) sends that
   exact placeholder value to a policy-matched endpoint. This means the
   `bash -c 'MCP_JSON="...$USER_ACCESS_TOKEN..."; claude ...'` pattern
   works by embedding the *placeholder* into the MCP config JSON — the
   swap happens later, at egress, not at shell-expansion time. Don't try
   to "resolve" the token yourself into a static file ahead of time.
5. **agent-proxy needs a wrapper script for MCP, not a raw `claude` command
   in `AGENT_COMMAND`** — building the MCP config JSON requires shell
   variable expansion (`$USER_ACCESS_TOKEN`) and quoting that agent-proxy's
   naive `split_whitespace()` `AGENT_COMMAND` parsing can't express. See
   the wrapper script in "Demo steps" above (`exec` replaces the shell's
   process image with `claude` — the policy's `--binary
   /usr/local/bin/claude` scoping still applies).
6. **`OUTPUT_FILE_FLAG=""`** — Claude's `-p` mode doesn't need a TTY (unlike
   codex), so agent-proxy's normal stdout-capture path works; the `-o`
   mechanism built for codex isn't needed here.
7. **Full round-trip verified**: `POST /v1/chat/completions` with "My
   mother is at the hospital, can I get an aid while I am on unpaid leave?"
   returned a detailed eligibility answer (Case A, 725€/month). Confirmed
   genuine (not a hallucination) by reading `mcp-server-a`'s own container
   logs directly: the `CallToolRequest` for `evaluate_unpaid_leave_eligibility`
   and its response include `"called_by": "user1"` and `"roles":
   ["openshell-user", "offline_access", "mcp-server-a-user"]` — proof the
   real Keycloak-derived JWT flowed through agent-proxy → Claude Code →
   the MCP server's own auth layer.

#### garak-envoy + full EvalHub pipeline (2026-08-18)

- Submitted a real EvalHub job (`quick` benchmark) through `garak-envoy`
  targeting `garak-claude-user1`'s agent-proxy — **completed successfully**
  with real `attack_success_rate`/`dan.Dan_11_0_asr` metrics (non-zero on a
  later retry, confirming genuine, non-deterministic LLM probe behavior —
  not a stub).
  scan) after the DeepSeek API key temporarily ran out of credits mid-run
  (`API Error: 402 Insufficient Balance`) — confirmed via a direct curl
  test through the same path while the job appeared to hang; the job
  itself never surfaced this clearly (silent retry/backoff), so a stalled
  job with zero CPU usage and zero new log lines for several minutes is a
  strong signal to check upstream API balance/quota directly.
- MLflow integration: this cluster already had a native RHOAI MLflow
  instance with most of the TrustyAI operator's wiring pre-done (CA cert,
  workspace, projected-SA-token mount, RBAC) — only `MLFLOW_TRACKING_URI`
  was missing. Patched the EvalHub CR's `spec.env` to set it; this
  triggers an EvalHub redeploy which cancels any in-flight job (confirmed
  — plan around this, don't redeploy mid-run). This finding is why
  Prerequisites step 3 now wires `MLFLOW_TRACKING_URI` in at CR creation
  time instead of patching it in afterward.

#### Full re-run under current Meridian naming, on a second fresh cluster (2026-08-26)

Re-ran the entire Claude Code path end to end (RHOAI 3.4.3) after the
Meridian Private Bank re-theme, using `alice`/`mcp-portfolio` instead of
the original `user1`/`mcp-server-a` — the first time this demo section was
tested against the current naming and against per-banker workspace
isolation (which didn't exist yet when the 2026-08-18 findings above were
recorded). Two things broke that the Prerequisites/Demo steps sections
above have since been corrected for:

- **The `agent-proxy` binary now installs from a real release, not a local
  build.** The GitHub Release workflow existed but had never actually been
  triggered — no `agent-proxy-v*` tag existed yet, so "download the
  prebuilt binary" silently had nothing to download. Cut `agent-proxy-v0.1.1`
  (root cause of the release being blocked: an untracked `.worktrees/`
  directory unrelated to this crate was tripping `cargo-release`'s
  repo-wide dirty-check — fixed by gitignoring it, not by working around
  the tool). Verified: the exact `curl` command in Prerequisites step 6
  downloads a working, static-pie-linked binary.
- **Every `openshell` command in the Demo steps needed `--workspace
  "${USER_ID}"` added** — the sandbox, both providers, the policy update,
  the upload/chmod calls, and `service expose`/`delete` all failed with
  "not found" or "provider not found" without it. The original recipe
  predates per-banker workspace isolation (each banker's provider,
  sandbox, and everything else now lives in their own dedicated
  workspace, not `default` — see the main guide's
  [Workspace isolation](../README.md#workspace-isolation)). This also
  changes the exposed service's `Host` header: it's
  `<workspace>--<sandbox-name>.openshell.localhost`, not
  `default--<sandbox-name>...` as originally documented.

With those two fixes applied, the full chain worked cleanly on the first
attempt: `garak-claude-alice` created in alice's workspace, `byo-claude` +
`user-alice` providers attached, agent-proxy uploaded and started,
service exposed as `alice--garak-claude-alice.openshell.localhost`. A
direct curl through the proxy asking "Who is my biggest client by assets
under management?" returned *"Your biggest client by assets under
management is **Elena Duarte** (client ID `cli-004`), with **$33,000** in
total AUM"* — confirmed genuine (not a hallucination) against
`mcp-portfolio`'s own container logs, which showed `"called_by": "alice"`
and her real role set (including `compatibility-user`, unique to her)
alongside the exact same `get_top_client_by_aum` result. Submitted the
`quick` Garak benchmark through `garak-envoy` — completed in under two
minutes with real metrics (`attack_success_rate: 0`, `dan.Dan_11_0_asr: 0`)
and an MLflow experiment reference in the response. Also found and fixed:
the Prerequisites' TrustyAI readiness check used a pod label
(`app=trustyai-operator`) that matches nothing on RHOAI 3.4.3 — the actual
pod is `trustyai-service-operator-controller-manager-*`, matched by
`control-plane=controller-manager,app.kubernetes.io/part-of=trustyai`.

### H. Open items

- [ ] Garak probe selection — the built-in `garak` provider has 8-12
  benchmarks depending on version. `evalhub providers describe garak` to
  list them. Pick benchmarks relevant to OpenShell's security layers
  (credential exfiltration, sandbox escape, unauthorized MCP access,
  token leakage) — **planned next step**: design a specific Garak
  probe/config set that actively tries to make the agent do things
  OpenShell's sandbox should protect against (see "Custom harm
  categories for OpenShell" above), then analyze results.
- [x] ~~Confirm Codex sandbox base image name and registry~~ —
  `quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012` (Codex 0.146.0)
- [x] ~~Verify how `sandbox exec` handles backgrounded processes~~ — see
  [Annex B](#b-troubleshooting).
- [x] ~~Confirm EvalHub's built-in Garak integration can target an arbitrary
  OpenAI-compatible endpoint URL~~ — **Yes**, via `garak-envoy` (see
  [Annex E](#e-evalhub-integration-background)).
- [x] ~~Confirm Claude Code base image~~ —
  `quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36` works (Claude Code
  2.1.220).
- [x] ~~Build and push custom images to `$GARAK_IMAGE_REGISTRY`~~ — both
  `codex-garak:latest` and `claude-garak:latest` pushed to
  `quay.io/atarazana/`. Use `make -C util/agent-proxy image-codex
  push-codex` (or `image-claude`/`push-claude`).
- [x] ~~Deploy EvalHub on the cluster~~ — done (see Prerequisites above).
- [x] ~~Verify that EvalHub's Garak adapter can reach the agent-proxy URL
  via the OpenShell gateway Route with Host-header routing~~ — it can't
  directly; `garak-envoy` (Prerequisites step 5) solves this. Confirmed
  working end-to-end.
- [x] ~~Wire up MLflow result tracking~~ — done (Prerequisites step 3),
  confirmed queryable via REST API.
- [ ] Verify that OpenShell gateway accepts JWTs from Keycloak client
  credentials grant [VERIFY] — if not, fall back to Playwright-based
  headless login with dedicated Keycloak users per evaluation profile
- [ ] Create representative Keycloak evaluation profiles — service accounts
  mirroring target user roles (e.g. MCP-server-A-only, both servers,
  no MCP). Grant matching realm roles and providers.
- [ ] Verify the browser MLflow UI's workspace-switching mechanism (only
  the REST API's `X-MLflow-Workspace` header was confirmed in this
  session).
- [ ] Design and run a Garak probe/collection specifically targeting
  OpenShell sandbox-escape and credential-exfiltration scenarios (see
  "Custom harm categories for OpenShell" above), then analyze results.

### I. Proposed file additions

#### As originally planned (2026-08-18, before implementation)

This was the plan before any code existed. Kept for historical reference —
see "Actual file layout" below for what was really built (they differ:
no `collections/` dir or numbered scripts were created; everything else
matches).

```
util/agent-proxy/
├── Cargo.toml
└── src/main.rs

demos/keycloak-oidc/
├── images/
│   ├── codex-garak/
│   │   ├── Containerfile
│   │   └── .gitignore          # excludes agent-proxy binary
│   └── claude-garak/
│       ├── Containerfile
│       └── .gitignore
├── collections/
│   └── openshell-redteam-v1.yaml
└── scripts/
    ├── 06-build-garak-image.sh      # phase 1: local build + push
    ├── 07-run-redteam-eval.sh
    └── ...
```

#### Actual file layout (as built)

```
util/agent-proxy/
├── Cargo.toml
├── Cargo.lock
├── Makefile              # build/musl/check/image/release targets
├── README.md
├── release.toml          # cargo-release config
└── src/main.rs

demos/keycloak-oidc/
├── garak-envoy/           # Helm chart — Host-header routing workaround
│   ├── Chart.yaml
│   ├── values.yaml
│   └── templates/
│       ├── configmap-envoy.yaml
│       ├── deployment.yaml
│       ├── service.yaml
│       └── route.yaml
├── images/
│   ├── codex-garak/
│   │   ├── Containerfile
│   │   └── .gitignore
│   └── claude-garak/
│       ├── Containerfile
│       └── .gitignore
└── docs/
    ├── evalhub-redteam.md       # this file
    ├── evalhub-redteam-orig.md  # original planning doc (superseded)
    └── diagrams/
        └── evalhub-redteam-architecture.svg  # beautified version of the ASCII diagram above

.github/workflows/
├── ci-agent-proxy.yml       # build + check on push/PR
└── release-agent-proxy.yml  # musl binary + both images on tag
```

No `collections/` dir or numbered scripts (`06-*.sh`, `07-*.sh`) exist —
all deployment steps are documented as copy-pasteable commands in this
guide instead (see "Prerequisites"/"Demo steps" above), matching how the
rest of `demos/keycloak-oidc/README.md` documents its recipes.

### J. Phases

#### Phase 1 — Local build (done)

- Build `agent-proxy` locally with `make -C util/agent-proxy musl`
- Build the custom sandbox image with `make -C util/agent-proxy
  image-codex` / `image-claude`, push with `push-codex`/`push-claude`
- Demo steps above cover running evaluations end to end

#### Phase 2 — CI automation (done)

- `.github/workflows/ci-agent-proxy.yml` — builds + checks on every push/PR
  touching `util/agent-proxy/`
- `.github/workflows/release-agent-proxy.yml` — on an `agent-proxy-v*` tag,
  builds the musl binary, builds + pushes both sandbox images to GHCR, and
  publishes a GitHub Release with the binary attached

### K. References

- EvalHub architecture:
  https://developers.redhat.com/articles/2026/05/12/how-evalhub-manages-two-layer-kubernetes-control-planes
- EvalHub BYOF docs:
  https://developers.redhat.com/articles/2026/06/09/bring-your-own-evaluation-framework-evalhub
- OpenShell service forwarding (gateway config):
  https://docs.nvidia.com/openshell/sandboxes/manage-gateways#configure-service-forwarding
- OpenShell expose long-running services:
  https://docs.nvidia.com/openshell/latest/sandboxes/manage-sandboxes#expose-long-running-services
- RHOAI 3.4 "Evaluating AI systems" (EvalHub + Garak + LM-Eval + Risk Assessment):
  docs.redhat.com/en/documentation/red_hat_openshift_ai_self-managed/3.4/html-single/evaluating_ai_systems
- TrustyAI Operator (deploys EvalHub): part of RHOAI DataScienceCluster, component set to Managed
- Garak (LLM vulnerability scanner): https://github.com/NVIDIA/garak
- OpenShell bring-your-own-container example:
  https://github.com/NVIDIA/OpenShell/tree/main/examples/bring-your-own-container
