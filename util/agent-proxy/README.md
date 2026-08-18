# agent-proxy — OpenAI-compatible bridge for CLI agents in a sandbox

## Why this tool exists

Red-team evaluation frameworks (Garak, EvalHub) speak an OpenAI-compatible
`/v1/chat/completions` HTTP API. Agentic CLIs like `codex` and `claude` are,
by design, CLIs — no HTTP endpoint. `agent-proxy` bridges the gap: a small
`axum` server, baked into or uploaded onto an OpenShell sandbox, that
accepts a chat-completion request, shells out to the configured agent CLI
with the last user message as the prompt, and returns the response in
OpenAI's response shape.

It runs **inside** the sandbox, not in front of it — so a red-team probe
hits the agent in the exact same environment a real user would have
(network policies, binary permissions, MCP server RBAC all live, not
simulated). See
[`demos/keycloak-oidc/docs/evalhub-redteam.md`](../../demos/keycloak-oidc/docs/evalhub-redteam.md)
for the full design, and its "TTY root cause" section for why getting a
CLI agent to run non-interactively inside a sandbox is less trivial than
it looks (codex specifically requires a real TTY on stdin/stdout/stderr).

## Getting the binary

### From GitHub Releases (recommended — no Rust toolchain needed)

Download the latest musl static binary directly:

```bash
curl -fsSL -o agent-proxy \
  https://github.com/alpha-hack-program/openshell-demos/releases/latest/download/agent-proxy-linux-x86_64-musl
chmod +x agent-proxy
```

This is exactly the artifact `.github/workflows/release-agent-proxy.yml`
builds — already musl static-linked, ready to `sandbox upload` as-is (see
"Deploying into a sandbox" below). No `cargo build` step needed.

### From source

Requires Rust 2024 edition (1.85+) and the `x86_64-unknown-linux-musl`
target (`rustup target add x86_64-unknown-linux-musl`).

```bash
cd util/agent-proxy
make musl        # builds target/x86_64-unknown-linux-musl/release/agent-proxy
```

**Always use the musl target for anything deployed into a sandbox** — a
plain `cargo build --release` (or `make release`) binary links against
whatever glibc your dev machine has, which is typically newer than the
sandbox base images ship (confirmed: Fedora 44's glibc 2.41 is too new).
`make release`/`make build` exist for local iteration only.

**Why static musl, not dynamic linking?** A glibc binary requires the
exact `GLIBC_x.xx` symbol versions it was linked against (or newer) to be
present on the machine that runs it — glibc only extends its ABI forward,
so a binary built against glibc 2.41 simply won't load on a system with
an older glibc. Since the build machine's glibc is typically newer than
whatever the sandbox base image ships (and we don't control that image's
glibc version, or want a matching build machine per image), the fix isn't
"link against an older glibc" — it's not linking against glibc at all.
musl's static linking bundles the entire libc into the binary, so it
carries no runtime dependency on the target's libc version — the same
binary runs unmodified on any of these Linux sandbox images regardless of
what they ship.

## Deploying into a sandbox

Two approaches — see
[`demos/keycloak-oidc/docs/evalhub-redteam.md`](../../demos/keycloak-oidc/docs/evalhub-redteam.md)
and the demo README's "Red-team evaluation with EvalHub + Garak" section
for the full walkthrough (sandbox creation, providers, network policy,
TTY handling, MCP wiring):

### Approach A — upload into a stock sandbox

```bash
make musl
openshell sandbox upload <sandbox> \
    target/x86_64-unknown-linux-musl/release/agent-proxy /sandbox/agent-proxy
```

### Approach B — bake into a custom image

```bash
make image-codex    # or image-claude
make push-codex      # or push-claude
```

This copies the musl binary into
`demos/keycloak-oidc/images/{codex,claude}-garak/` (gitignored there) and
runs `podman build -f`. `GARAK_IMAGE_REGISTRY` defaults to
`quay.io/atarazana` — override it: `make image-codex GARAK_IMAGE_REGISTRY=quay.io/yourorg`.

## Configuration

All configuration is via environment variables set on the running process
(e.g. `openshell sandbox exec --env KEY=VALUE`) — the `--help` defaults are
for local/manual runs only; the HTTP handler reads `std::env::var(...)`
directly, not the parsed CLI args.

| Env var | Default | Description |
|---|---|---|
| `AGENT_COMMAND` | `codex` (handler fallback) | Command + base args to run. The user prompt is appended as the final argument. Split naively on whitespace — for commands needing shell quoting (e.g. Claude Code's `--mcp-config` with a runtime-expanded token), point this at a wrapper script instead. |
| `OUTPUT_FILE_FLAG` | `-o` | Flag telling the agent to write its final response to a file (codex's `-o`/`--output-last-message`), passed as `<flag> <tmpfile>` before the prompt. When set, stdin/stdout/stderr are all inherited from agent-proxy's own process (needed because codex requires all three to be real TTYs — see "TTY root cause" in the design doc). Set to an empty string to disable and capture stdout directly instead (e.g. for Claude Code's `-p` mode, which doesn't require a TTY). |
| `--host` / `--port` (flags, not env) | `0.0.0.0` / `8080` | Listen address |

### Agent-specific examples

| Agent | `AGENT_COMMAND` | `OUTPUT_FILE_FLAG` |
|---|---|---|
| Codex | `codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox` | `-o` (default) |
| Claude Code | path to a wrapper script (see demo README) | `""` (disabled) |

Codex additionally needs a real TTY on stdin/stdout/stderr, which means
agent-proxy itself must be started via a **foreground**
`openshell sandbox exec --tty` (backgrounded on the *local* machine, not
with `nohup &` inside the sandbox — that never requests a pty and tears
down the exec channel immediately). Claude Code's `-p` mode has no such
requirement; a plain background start works.

## Development

```bash
make help       # list all targets
make check      # fmt-check + clippy
make build      # debug build (native target, for fast local iteration)
```

## Releasing

Requires [`cargo-release`](https://github.com/crate-ci/cargo-release):

```bash
cargo install cargo-release
make release-patch   # or release-minor / release-major
```

This bumps `Cargo.toml`, runs `make ci` as a gate, commits, tags
`agent-proxy-v<version>`, and pushes. GitHub Actions then builds the musl
binary and both sandbox images (`codex-garak`, `claude-garak`) and
publishes a GitHub Release with the binary attached.
