# EvalHub Red-Team Demo — Draft Implementation Plan

Status: **DRAFT** — open for discussion, not yet in design phase.

## Purpose

Show how to run repeatable, auditable red-team evaluations against AI agents
(Codex / Claude Code) running inside OpenShell sandboxes — using EvalHub as
the orchestrator (via RHOAI UI or CLI), Garak as the adversarial probe
engine, and a Rust proxy (`agent-proxy`) baked into the sandbox image to
bridge Garak's OpenAI-compatible API expectations to the CLI-based agent.

> **Claude Code is the primary path for this demo; Codex is optional.**
> Against the demo's default DeepSeek BYO backend, Codex can only exercise
> model-only red-team probes — DeepSeek rejects the `namespace` tool type
> Codex uses for MCP, so Codex + MCP evaluations don't work here (see "TTY
> root cause" below). Codex + MCP only works against an on-cluster vLLM
> **≥0.25.0** (upstream; RHOAI 3.4.x ships 0.18.0, which is too old — see
> `docs/inference-api-compatibility.md`). Claude Code's Anthropic Messages
> API has no such restriction and is validated end-to-end against DeepSeek,
> including MCP tool use (see "Claude Code + MCP via agent-proxy" below).

## Architecture

```
  RHOAI UI / evalhub CLI
         │
         ▼
  EvalHub Server ───► Garak Job (K8s Job)
         │                    │
         │                    ▼
         │         openshell service expose URL
         │                    │
         │  ┌─ OpenShell sandbox (custom image) ──────────┐
         │  │                                              │
         │  │  ┌──────────────┐                            │
         │  │  │ agent-proxy  │◄── :8080 (OpenAI-compat)   │
         │  │  │ (Rust, baked │                            │
         │  │  │  into image) │                            │
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

### Key insight

The agent-proxy runs **inside** the sandbox, not in front of it. Garak's
adversarial probes hit the agent in the exact same environment a real user
would have — network policies, binary permissions, MCP server RBAC are all
live, not simulated. The proxy is exposed only on demand via
`openshell service expose`.

## Resolved design decisions

| Question | Decision | Rationale |
|---|---|---|
| Proxy name / location | `agent-proxy` at `util/agent-proxy/` (repo root) | Reusable across demos, consistent with `util/onboard/` |
| Agent selection | Claude Code (primary), Codex (optional) | Both supported via `AGENT_COMMAND` env var; separate Containerfile per agent. Codex + MCP requires an on-cluster vLLM ≥0.25.0 — against the demo's default DeepSeek backend it's model-only-probes-only. Claude Code works fully (incl. MCP) against DeepSeek. |
| Sandbox naming (this demo section) | `garak-codex-<user>` / `garak-claude-<user>` | Distinguishes agent type in the sandbox name (e.g. `garak-codex-user1`, `garak-claude-user1`) — avoids ambiguity with the main demo's `demo-<user>` sandboxes and with each other. |
| EvalHub MCP server | Not needed | Evaluations driven from RHOAI UI / CLI, not from inside sandboxes |
| BYOF adapter | Not needed | agent-proxy already exposes an OpenAI-compatible endpoint; EvalHub's built-in `garak` provider accepts any `model.url` pointing to an OpenAI `/v1` endpoint. The `garak-kfp` risk assessment pipeline (Ch. 4) also accepts arbitrary URLs. **Confirmed from RHOAI 3.4 docs.** |
| EvalHub integration path | Built-in `garak` provider first (Path 1), `garak-kfp` risk assessment later (Path 2) | Path 1 needs only EvalHub + agent-proxy URL. Path 2 adds multi-strategy attacks (SPO, Translation, TAP) but requires KFP + S3 + judge/SDG models — heavier infrastructure. |
| Demo structure | Extend `demos/keycloak-oidc/` | Same infrastructure stack (OIDC, Envoy, per-user sandboxes); avoids duplication |
| Sandbox image | Two approaches: **A) Custom image** with agent-proxy baked in, or **B) Upload** the binary into a stock sandbox at runtime | **A** is best for CI/reproducibility and use cases where the binary must always be present (e.g. Prometheus exporters). **B** (upload+expose) is simpler for ad-hoc runs — no image build, no registry, no Containerfile. Both validated on a live cluster. |
| Proxy startup | Create sandbox first, then start proxy via a **foreground** `sandbox exec --tty` (backgrounded on the *local* machine, not with `nohup &` inside the sandbox) | The `-- <command>` on `sandbox create` runs via SSH and blocks the CLI. codex requires stdin/stdout/stderr to all be a real TTY (see "TTY root cause" below); `nohup agent-proxy &` inside the sandbox exits the wrapping shell immediately and tears down the pty. `openshell sandbox exec -n <sandbox> --tty -- agent-proxy ... &` (local `&`) keeps the exec channel — and its pty — alive for the proxy's whole lifetime. **Confirmed on live cluster** (2026-08-18). |
| Service exposure | `openshell service expose <sandbox> 8080` (not `--forward`) | `--forward` creates a local-only SSH tunnel; `service expose` creates a gateway-managed HTTPS URL using Host-header routing through the gateway's Route. Garak reaches it via `curl -H "Host: <svc-host>" https://<route>/`. **Confirmed.** |
| Image build for remote gateways | `podman build` + `podman push` + `--from <image-ref>` | `--from <Dockerfile-dir>` only works for local gateways. For remote (OpenShift), build/push manually to `$GARAK_IMAGE_REGISTRY`. **Confirmed.** |
| Containerfile naming | `Containerfile` (Podman-idiomatic) | The OpenShell CLI's `--from <directory>` only looks for `Dockerfile`, but that mode doesn't work on remote gateways anyway. We build with `podman build -f`, which accepts any name. **Confirmed.** |
| Garak probe selection | Deferred | Will survey Garak's catalog later |

## Components

### 1. `agent-proxy` — Rust binary (`util/agent-proxy/`)

A small `axum` server exposing `POST /v1/chat/completions`. On each request:

- Extracts the last user message from the `messages` array
- Shells out to the configured agent CLI via `AGENT_COMMAND` env var
- Captures the response (via a `-o`/output-file flag, or stdout — see
  "TTY root cause" below)
- Returns a standard OpenAI `ChatCompletion` response

The `AGENT_COMMAND` is split on whitespace; the user prompt is appended as
the final argument. Defaults and required flags per agent:

| Agent | `AGENT_COMMAND` value | `OUTPUT_FILE_FLAG` |
|---|---|---|
| Codex (default) | `codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox` | `-o` (default) |
| Claude Code | `claude --mcp-config /sandbox/.claude.json --strict-mcp-config --permission-mode bypassPermissions --output-format text -p` | `""` (disabled — `-p` mode prints plain text to stdout and doesn't require a TTY) |

For Claude Code, `-p` must be the last flag so the appended prompt becomes
its value. The MCP config file must be uploaded into the sandbox at creation
time (same `--upload` pattern as the Codex config.toml).

`AGENT_COMMAND` must be set as a real environment variable on the running
process (e.g. via `sandbox exec --env`) — the compiled-in default only
applies to `--help` text, not to request handling, since the HTTP handler
reads `std::env::var("AGENT_COMMAND")` directly.

No streaming needed — Garak sends blocking requests. No auth on the proxy
itself — it's loopback inside a sandbox, exposed only via
`openshell service expose`.

Built with Cargo. Released as a static binary.

### TTY root cause (resolved 2026-08-18) — Codex path, optional

> Codex is optional for this demo (see the note under Purpose) — this
> section documents the TTY fix for the model-only-probes case, and the
> vLLM ≥0.25.0 requirement for Codex + MCP.

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

### 2. Deploying agent-proxy into a sandbox

Two approaches, both validated on a live cluster:

#### Approach A — Upload into a stock sandbox (simpler)

No custom image, no registry, no Containerfile. Use the stock agent base
image and upload the binary at runtime:

```bash
# 1. Build the static binary
cargo build --release --target x86_64-unknown-linux-musl \
    --manifest-path util/agent-proxy/Cargo.toml

# 2. Create a sandbox from the stock base image
openshell sandbox create --name garak-codex-user1 \
    --from quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012 \
    -- true

# 3. Upload the binary
openshell sandbox upload -n garak-codex-user1 \
    util/agent-proxy/target/x86_64-unknown-linux-musl/release/agent-proxy \
    /usr/local/bin/agent-proxy

# 4. Make it executable
openshell sandbox exec -n garak-codex-user1 -- chmod +x /usr/local/bin/agent-proxy

# 5. Start in the FOREGROUND with --tty (codex needs a real TTY on
#    stdin/stdout/stderr — `nohup ... &` inside the sandbox tears it down).
#    Background the exec call itself on the LOCAL machine instead.
openshell sandbox exec -n garak-codex-user1 --tty \
    --env 'AGENT_COMMAND=codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox' \
    -- /usr/local/bin/agent-proxy --port 8080 > proxy-exec.log 2>&1 &
PROXY_EXEC_PID=$!

# 6. Expose
openshell service expose garak-codex-user1 8080
```

Best for: ad-hoc red-team runs, local testing, demos where you don't want
to manage a registry. Also the easiest path for evaluating different
agent-proxy versions — just upload and restart.

#### Approach B — Custom sandbox image (Containerfile)

Following the [bring-your-own-container](https://github.com/NVIDIA/OpenShell/tree/main/examples/bring-your-own-container)
pattern. Requirements: non-root user, `/sandbox` workspace, `iproute2`
installed. OpenShell replaces `CMD`/`ENTRYPOINT` with its sandbox supervisor;
the `-- <command>` is executed **via SSH once the sandbox is ready**, so the
binary must be baked into the image.

One Containerfile per agent type, each layering `agent-proxy` onto the
agent's sandbox image:

| Agent | Base image | Containerfile |
|---|---|---|
| Codex | `quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012` | `demos/keycloak-oidc/images/codex-garak/Containerfile` |
| Claude Code | `quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36` | `demos/keycloak-oidc/images/claude-garak/Containerfile` |

Build and push manually with `podman build -f` (the CLI's `--from <dir>`
only works for local gateways — on remote gateways, pass the image
reference directly):

```bash
GARAK_IMAGE_REGISTRY="${GARAK_IMAGE_REGISTRY:-quay.io/atarazana}"

# Codex
podman build -t "${GARAK_IMAGE_REGISTRY}/codex-garak:latest" \
    -f demos/keycloak-oidc/images/codex-garak/Containerfile \
    demos/keycloak-oidc/images/codex-garak/
podman push "${GARAK_IMAGE_REGISTRY}/codex-garak:latest"
openshell sandbox create --name garak-codex-user1 \
    --from "${GARAK_IMAGE_REGISTRY}/codex-garak:latest"

# Claude Code
podman build -t "${GARAK_IMAGE_REGISTRY}/claude-garak:latest" \
    -f demos/keycloak-oidc/images/claude-garak/Containerfile \
    demos/keycloak-oidc/images/claude-garak/
podman push "${GARAK_IMAGE_REGISTRY}/claude-garak:latest"
openshell sandbox create --name garak-claude-user1 \
    --from "${GARAK_IMAGE_REGISTRY}/claude-garak:latest"
```

### 3. Collection YAML

Garak probe/detector config for EvalHub. Probe selection is deferred — will
be defined based on a survey of Garak's catalog targeting the security layers
in play (credential exfiltration, sandbox escape, unauthorized MCP access,
token leakage).

### 4. Scripts (extend `demos/keycloak-oidc/scripts/`)

New numbered scripts continuing the existing sequence. Each script should
support both deployment approaches (upload vs custom image) via an env var
or flag.

#### Approach A — Upload workflow

- Build agent-proxy (static binary only, no image build):
  ```bash
  cargo build --release --target x86_64-unknown-linux-musl \
      --manifest-path util/agent-proxy/Cargo.toml
  ```
- Create sandbox from stock image, upload binary, start proxy, expose:
  ```bash
  AGENT_IMAGE="quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012"
  openshell sandbox create --name garak-codex-user1 --from "$AGENT_IMAGE" -- true
  openshell sandbox upload -n garak-codex-user1 \
      util/agent-proxy/target/x86_64-unknown-linux-musl/release/agent-proxy \
      /usr/local/bin/agent-proxy
  openshell sandbox exec -n garak-codex-user1 -- chmod +x /usr/local/bin/agent-proxy
  # Foreground + --tty (see "TTY root cause" above); backgrounded locally.
  openshell sandbox exec -n garak-codex-user1 --tty \
      --env 'AGENT_COMMAND=codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox' \
      -- /usr/local/bin/agent-proxy --port 8080 > proxy-exec.log 2>&1 &
  PROXY_EXEC_PID=$!
  openshell service expose garak-codex-user1 8080
  ```

#### Approach B — Custom image workflow

- Build agent-proxy and the custom image, push to registry:
  ```bash
  GARAK_IMAGE_REGISTRY="${GARAK_IMAGE_REGISTRY:-quay.io/atarazana}"
  cargo build --release --manifest-path util/agent-proxy/Cargo.toml
  cp util/agent-proxy/target/release/agent-proxy demos/keycloak-oidc/images/codex-garak/
  podman build -t "${GARAK_IMAGE_REGISTRY}/codex-garak:latest" demos/keycloak-oidc/images/codex-garak/
  podman push "${GARAK_IMAGE_REGISTRY}/codex-garak:latest"
  ```
  **Note:** `--from <directory>` on `sandbox create` only works for local
  gateways. For remote gateways (OpenShift), build and push the image
  manually, then pass the full image reference to `--from`.
- Create sandbox from the pushed image:
  ```bash
  openshell sandbox create --name garak-codex-user1 \
      --from "${GARAK_IMAGE_REGISTRY}/codex-garak:latest" \
      -- true
  ```
- Start agent-proxy via a **foreground** `sandbox exec --tty`, backgrounded
  on the local machine (codex requires a real TTY on stdin/stdout/stderr —
  see "TTY root cause" above; `nohup ... &` inside the sandbox tears down
  the pty). Stop it by killing the local exec PID (`kill $PROXY_EXEC_PID`)
  or `pkill -f agent-proxy` inside the sandbox as a fallback:
  ```bash
  openshell sandbox exec -n <sandbox> --tty \
      --env 'AGENT_COMMAND=codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox' \
      -- /usr/local/bin/agent-proxy --port 8080 > proxy-exec.log 2>&1 &
  PROXY_EXEC_PID=$!
  ```
- Expose the proxy via a gateway-managed URL (not local `--forward`, since
  Garak runs as a K8s Job on the cluster and needs a reachable endpoint).
  The URL uses Host-header routing through the gateway's Route — Garak
  reaches it via `curl -H "Host: <service-host>" https://<gateway-route>/`:
  ```bash
  openshell service expose <sandbox> 8080
  ```
- Trigger the EvalHub evaluation (via CLI) pointing Garak at the exposed URL
- Collect / display results
- Clean up: `openshell service delete <sandbox>`, then
  `openshell sandbox delete <sandbox>`

## Phases

### Phase 1 — Local build

- Build `agent-proxy` locally with `cargo build --release`
- Build the custom sandbox image with `podman build`, push to
  `$GARAK_IMAGE_REGISTRY` (default `quay.io/atarazana`)
- Pass the full image reference to `--from` on `sandbox create`
- Write and test the demo scripts
- Run evaluations end to end

### Phase 2 — CI automation

- GitHub Actions workflow triggered on changes to `util/agent-proxy/` or
  the Containerfile
- Compiles `agent-proxy` via Cargo
- Builds and pushes the custom sandbox image to GHCR
- Follows the existing release workflow pattern in this repo

## Proposed file additions

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

## Validated — proxy lifecycle (2026-08-18)

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
6. **`sandbox upload` + `sandbox exec`** — used to test the proxy in an
   existing sandbox without building a custom image. Works.
7. **Background startup — SUPERSEDED.** `sandbox exec -- bash -c 'nohup
   agent-proxy &'` starts the proxy and the exec call returns, but this
   pattern is incompatible with codex: it never requests a pty (no
   `--tty`) and the wrapping shell exits immediately, tearing down the
   exec channel anyway. See "TTY root cause" above — use a **foreground**
   `sandbox exec --tty`, backgrounded on the local machine, instead. Still
   fine for agents that don't need a TTY (e.g. Claude Code's `-p` mode).
8. **`service expose`** — creates a gateway-managed HTTPS URL using
   Host-header routing through the gateway's Route. Garak K8s Jobs reach
   the proxy via `curl -H "Host: <service-host>" https://<gateway-route>/`.
9. **Full round-trip** — `POST /v1/chat/completions` through the gateway
   returns a valid OpenAI `ChatCompletion` response. Confirmed with an
   `echo` stub and a Claude Code invocation (Claude correctly fails with
   exit 1 when no API key is configured — proxy surfaces the error).
10. **Cleanup** — `openshell service delete <sandbox>` removes the exposed
    service.
11. **Codex TTY fix, full round-trip confirmed (2026-08-18)** — with the
    `TERM`/stdin fix, foreground `--tty` startup, and `-o` output-file
    capture (see "TTY root cause" above), `POST /v1/chat/completions` →
    agent-proxy → codex → a real LLM response works end-to-end through the
    gateway route. Verified twice on the live `garak-codex-user1` sandbox
    (`keycloak-oidc-demo` namespace) with distinct prompts, including one
    requiring actual computation (`17 * 23` → `391`), ruling out a stubbed
    or cached response.
12. **Claude Code + MCP through agent-proxy, full round-trip confirmed
    (2026-08-18)** — see "Claude Code + MCP via agent-proxy" below.

## Claude Code + MCP via agent-proxy (validated 2026-08-18)

Unlike codex on DeepSeek (see "TTY root cause" above — DeepSeek rejects
Codex's `namespace` tool type), Claude Code's Anthropic Messages API uses
standard tool definitions, so MCP tool use works against the demo's DeepSeek
BYO backend. Confirmed end-to-end on a live cluster:

1. **Sandbox**: created `garak-claude-user1` from the stock Claude Code base image
   (`quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36`) — `garak-codex-user1`
   (Codex base image) has no `claude` binary, so a separate sandbox is
   needed for this path.
2. **Provider**: `byo-claude` didn't exist yet — created it from
   `providers/byo-claude-profile.yaml` (LLM host substituted with
   `api.deepseek.com`), then `openshell sandbox provider attach garak-claude-user1
   byo-claude` and `... user-user1` (for `USER_ACCESS_TOKEN`, needed for MCP
   auth).
3. **Policy**: granted `garak-claude-user1` network access to the LLM host and
   `mcp-server-a.<namespace>.svc.cluster.local:8000`, both scoped to
   `--binary /usr/local/bin/claude` — same pattern as the README's
   `demo-${USER_ID}` recipe.
4. **Credential resolution is network-layer, not env-var-layer.** A plain
   `sandbox exec -- bash -c 'echo $USER_ACCESS_TOKEN'` prints the literal
   placeholder string (`openshell:resolve:env:...`), **not** the real
   token — confirmed empirically. The real secret is substituted
   transparently when the bound binary (`/usr/local/bin/claude`) sends that
   exact placeholder value to a policy-matched endpoint. This means the
   README's `bash -c 'MCP_JSON="...$USER_ACCESS_TOKEN..."; claude ...'`
   pattern works by embedding the *placeholder* into the MCP config JSON —
   the swap happens later, at egress, not at shell-expansion time. Don't
   try to "resolve" the token yourself into a static file ahead of time.
5. **agent-proxy needs a wrapper script for MCP, not a raw `claude` command
   in `AGENT_COMMAND`** — building the MCP config JSON requires shell
   variable expansion (`$USER_ACCESS_TOKEN`) and quoting that agent-proxy's
   naive `split_whitespace()` `AGENT_COMMAND` parsing can't express. Upload
   a small wrapper (e.g. `/sandbox/run-claude.sh`) that builds the JSON and
   `exec`s `claude` with it, and set `AGENT_COMMAND=/sandbox/run-claude.sh`:
   ```bash
   #!/bin/bash
   set -e
   MCP_JSON="{\"mcpServers\":{\"eligibility\":{\"type\":\"http\",\"url\":\"http://mcp-server-a.<namespace>.svc.cluster.local:8000/mcp\",\"headers\":{\"Authorization\":\"Bearer $USER_ACCESS_TOKEN\"}}}}"
   exec claude -p "$1" \
     --mcp-config "$MCP_JSON" \
     --strict-mcp-config \
     --permission-mode bypassPermissions \
     --output-format text
   ```
   (`exec` replaces the shell's process image with `claude` — the policy's
   `--binary /usr/local/bin/claude` scoping still applies.)
6. **`OUTPUT_FILE_FLAG=""`** — Claude's `-p` mode doesn't need a TTY (unlike
   codex), so agent-proxy's normal stdout-capture path works; the `-o`
   mechanism built for codex isn't needed here.
7. **Start agent-proxy** with `AGENT_COMMAND=/sandbox/run-claude.sh`,
   `OUTPUT_FILE_FLAG=` (empty), and the `ANTHROPIC_BASE_URL` /
   `ANTHROPIC_MODEL` / `ANTHROPIC_DEFAULT_*_MODEL` env vars from the
   README's Claude Code recipe — plain background start is fine (no `--tty`
   needed).
8. **Full round-trip verified**: `POST /v1/chat/completions` with "My
   mother is at the hospital, can I get an aid while I am on unpaid leave?"
   returned a detailed eligibility answer (Case A, 725€/month). Confirmed
   genuine (not a hallucination) by reading `mcp-server-a`'s own container
   logs directly: the `CallToolRequest` for `evaluate_unpaid_leave_eligibility`
   and its response include `"called_by": "user1"` and `"roles":
   ["openshell-user", "offline_access", "mcp-server-a-user"]` — proof the
   real Keycloak-derived JWT flowed through agent-proxy → Claude Code →
   the MCP server's own auth layer.

## Open items

- [ ] Garak probe selection — the built-in `garak` provider has 12 benchmarks.
  List them with `evalhub providers describe garak` once EvalHub is deployed.
  Pick benchmarks relevant to OpenShell's security layers (credential
  exfiltration, sandbox escape, unauthorized MCP access, token leakage).
  For custom OpenShell-specific categories, use Path 2 (risk assessment
  with custom harm categories via policy dataset)
- [x] ~~Confirm Codex sandbox base image name and registry~~ —
  `quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012` (Codex 0.146.0)
- [x] ~~Verify how `sandbox exec` handles backgrounded processes~~ —
  `bash -c 'nohup agent-proxy &'` returns immediately, but doesn't carry a
  pty through to codex. Use a **foreground** `sandbox exec --tty`,
  backgrounded locally, when the agent needs a TTY (codex); `pkill -f
  agent-proxy` remains a valid fallback to stop it either way. See "TTY
  root cause" above.
- [x] ~~Confirm EvalHub's built-in Garak integration can target an arbitrary
  OpenAI-compatible endpoint URL~~ — **Yes.** The `model.url` field in the
  EvalHub job submission accepts any OpenAI `/v1`-compatible URL. Both the
  built-in `garak` provider (Chapter 2) and the `garak-kfp` risk assessment
  pipeline (Chapter 4) use this field. See "EvalHub integration" section
  below for details.
- [x] ~~Confirm Claude Code base image~~ —
  `quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36` works (Claude Code
  2.1.220). Sandbox reaches Ready, agent-proxy runs, service expose works.
- [x] ~~Build and push custom images to `$GARAK_IMAGE_REGISTRY`~~ —
  both `codex-garak:latest` and `claude-garak:latest` pushed to
  `quay.io/atarazana/`. Must use `flatpak-spawn --host podman` from inside
  a toolbox, or build on the host directly. Binary must be statically
  linked (musl target).
- [x] ~~Deploy EvalHub on the cluster~~ — already deployed on this cluster
  (TrustyAI/KServe `Managed`, `evalhub` namespace, `garak` provider,
  `safety-and-fairness-v1` collection, CLI configured). No action needed.
- [x] ~~Verify that EvalHub's Garak adapter can reach the agent-proxy URL
  via the OpenShell gateway Route with Host-header routing~~ — **No, it
  can't.** Confirmed broken on a live cluster (`openai.NotFoundError: 404`)
  — see "CONFIRMED BROKEN" note above. EvalHub's `ModelConfig` has no
  headers field; Garak's OpenAI generator sends no custom `Host`. Blocking
  issue for using the built-in `garak` provider against any
  `service expose`d sandbox. Needs one of the two fixes listed there
  before this path is usable.
- [ ] Verify that OpenShell gateway accepts JWTs from Keycloak client
  credentials grant [VERIFY] — if not, fall back to Playwright-based
  headless login with dedicated Keycloak users per evaluation profile
- [ ] Create representative Keycloak evaluation profiles — service accounts
  mirroring target user roles (e.g. MCP-server-A-only, both servers,
  no MCP). Grant matching realm roles and providers.
- [ ] Phase 2: design the GitHub Actions workflow

## Roles and responsibilities

### Who runs what

Red-team evaluation is a **secops / AI-secops function**, not a user
responsibility. Users don't audit their own sandboxes — that's a conflict
of interest. The full lifecycle (create sandbox → deploy proxy → expose →
run Garak → collect results → cleanup) is owned by a secops role or
automated by a service account.

| Duty | Who | Why |
|---|---|---|
| Deploy EvalHub (Operator, PostgreSQL, CR) | Cluster admin | Cluster-scoped infrastructure |
| Build + push custom images (Approach B) | Secops / CI | Registry and build pipeline access |
| Build agent-proxy static binary | Secops / CI | Cargo + musl toolchain |
| Create sandbox, upload/start proxy, expose service | **Secops service account** | Red-teaming is not a user duty |
| Submit EvalHub evaluation jobs | **Secops service account** | Owns the evaluation results and compliance reports |
| Review results, file findings | Secops | Owns the security posture |
| Define custom harm categories | Secops | Domain-specific policy |

### Security context challenge

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

### Representative user profiles

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

### Automated evaluation loop

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
    #    locally — see "TTY root cause" above), expose
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

### OIDC authentication for service accounts — [VERIFY]

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
[`docs/headless-browser-automation.md`](headless-browser-automation.md).

### Demo shortcut

The production approach (service accounts per profile) is the right design
but overkill for a demo. For the `keycloak-oidc` demo, **use an existing
demo user** (`user1` or `user2`) who is already authenticated with the
correct providers and MCP roles:

1. Authenticate as `user1` (already done if following the demo README)
2. Create sandbox, upload agent-proxy, start, expose — as `user1`
3. Submit the EvalHub evaluation pointing at the exposed URL
4. Collect results, clean up

The sandbox gets `user1`'s exact security context — providers, MCP roles,
network policies — which is precisely what we want to red-team. The demo
README should note that in production this would be driven by a secops
service account with representative profiles, not by the end user.

## EvalHub integration

Based on the RHOAI 3.4 "Evaluating AI systems" documentation (2026-06-04).

### Two paths to Garak in EvalHub

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

### Path 1 — Built-in `garak` provider

**Prerequisites:**
- TrustyAI Operator installed, `DataScienceCluster` component set to `Managed`
- KServe in `RawDeployment` mode
- PostgreSQL database for EvalHub
- EvalHub CR deployed (with `garak` in `spec.providers`)

**Deploy EvalHub (outline):**

1. Create a namespace for EvalHub (not `redhat-ods-applications` — NetworkPolicies
   restrict cross-namespace traffic)
2. Create the PostgreSQL connection Secret (`evalhub-db-credentials`)
3. Create the EvalHub CR:
   ```yaml
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
   ```
4. `oc apply -f evalhub_cr.yaml -n <evalhub-namespace>`
5. Verify: `oc get pods -l app=eval-hub -n <evalhub-namespace>`

**Install the CLI:**
```bash
pip install "eval-hub-sdk[cli]"
evalhub config set base_url https://$(oc get routes evalhub -o jsonpath='{.spec.host}' -n <evalhub-namespace>)
evalhub config set tenant <evalhub-namespace>
export TOKEN=$(oc create token <serviceaccount> -n <evalhub-namespace>)
evalhub config set token $TOKEN
evalhub health
```

**Submit an evaluation job targeting agent-proxy:**

The model URL is the `service expose` URL from the sandbox, accessed via
Host-header routing:

```bash
# Derive the agent-proxy URL
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
SANDBOX_NAME="garak-codex-user1"  # or garak-claude-user1
PROXY_URL="https://${ROUTE_HOST}"

# Submit via CLI
evalhub eval run \
    --name redteam-codex-v1 \
    --model-url "$PROXY_URL" \
    --model-name "codex-agent-proxy" \
    --provider garak \
    -b <benchmark_id>

# Or submit via REST API
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
`garak-claude-user1`'s exposed agent-proxy. The job's adapter pod failed
immediately with `openai.NotFoundError: Error code: 404`.

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
EvalHub SDK version** — not a missing flag, an architecture gap. Two ways
forward, neither attempted yet:
1. **Bypass `service expose` entirely** — create a dedicated Kubernetes
   `Service` (selecting the sandbox pod directly by label) + `Route` with
   its own unique hostname, so `model-url` needs no Host override. Loses
   the "proxy exposed only on demand" isolation `service expose` gives,
   and requires knowing/relying on the sandbox pod's labels (not a stable,
   documented OpenShell API).
2. **File an OpenShell and/or EvalHub feature request** — either
   `service expose` should support a real per-sandbox hostname (its own
   Route, not Host-header multiplexing on the shared one), or EvalHub's
   `ModelConfig` should accept custom headers for the target model
   endpoint.
Until one of these lands, the built-in `garak` provider (Path 1) cannot
target an OpenShell sandbox's `agent-proxy` on this cluster. The
`garak-kfp` risk assessment pipeline (Path 2) hasn't been tested against
this constraint and may have the same issue (same `ModelRef`-style schema
per the RHOAI docs).

**Track results:**
```bash
evalhub eval status <job_id>
evalhub eval results <job_id>
```

### Viewing results in the RHOAI dashboard / MLflow (validated 2026-08-18)

**EvalHub/Garak results have no native RHOAI dashboard view** — the
dashboard's "Performing model evaluations in the dashboard" feature (RHOAI
3.4 docs §3.5) is scoped to **LM-Eval only**. The only UI-capable path for
EvalHub is MLflow experiment tracking.

This cluster already has a native RHOAI MLflow instance (deployed via the
`mlflow.opendatahub.io` operator in `redhat-ods-applications`, exposed at
`https://<rhoai-dashboard-domain>/mlflow`), and the TrustyAI operator
**pre-wires most of the integration automatically** — the EvalHub
deployment already had `MLFLOW_CA_CERT_PATH`, `MLFLOW_WORKSPACE`, and a
projected-ServiceAccount-token `MLFLOW_TOKEN_PATH` mount (with matching
RBAC already granted) before we touched anything. The **only** missing
piece was `MLFLOW_TRACKING_URI` — unset by default, which is what disables
MLflow integration entirely per RHOAI docs §2.22.3. Enable it:

```bash
oc patch evalhub evalhub -n "$EVALHUB_NAMESPACE" --type=merge -p \
  '{"spec":{"env":[{"name":"MLFLOW_TRACKING_URI","value":"https://mlflow.redhat-ods-applications.svc:8443"}]}}'

oc rollout status deployment/evalhub -n "$EVALHUB_NAMESPACE"
```

This triggers a redeploy (which cancels any job currently running — submit
new jobs only after `evalhub health` reports healthy again). Then pass
`--experiment <name>` on every job submission to log it:

```bash
evalhub eval run \
  --name "redteam-${USER_ID}" \
  --model-url "https://${GARAK_ENVOY_HOST}/route/${SERVICE_HOST}" \
  --model-name "${SANDBOX}-agent-proxy" \
  --provider garak \
  -b owasp_llm_top10 \
  --experiment "redteam-${USER_ID}"
```

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

### Path 2 — Automated Risk Assessment (future)

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

This path is deferred until Path 1 is validated end-to-end.

### Custom harm categories for OpenShell

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
of these (particularly credential exfiltration and security/malware).

## References

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
