# session-auditor — tamper-resistant compliance/risk auditing for a sandbox

One crate, one binary, shared across agents — built into two images,
[`claude-audit`](../../demos/keycloak-oidc/images/claude-audit/) and
[`codex-audit`](../../demos/keycloak-oidc/images/codex-audit/). Registered
as three separate hooks — `SessionStart`, `UserPromptSubmit`, and `Stop` —
all of which share the same core stdin fields (`session_id`,
`hook_event_name`, and, for `Stop` only, `transcript_path`), confirmed live
for both agents — only the hook *registration* mechanism and the
transcript's own on-disk format differ per agent, both handled inside this
one binary (see "Multi-agent support" below).

## Why this tool exists

A metric produced from inside a sandbox is worth nothing if the sandbox's
own user (or an agent acting on their behalf) can just fake it — and
OpenShell's `filesystem_policy` (Landlock) has no per-binary scoping, so a
`read_only` rule can't distinguish "the agent wrote this" from "the user
typed the same command via `sandbox exec`." `session-auditor` sidesteps
that with three combined protections, none of which are optional the way
the deployment approach is for `util/agent-proxy`:

1. It's registered as `SessionStart`/`UserPromptSubmit`/`Stop` hooks via a
   managed, non-user-overridable config file —
   `/etc/claude-code/managed-settings.json` for Claude Code,
   `/etc/codex/requirements.toml` for Codex. Both agents' own managed-config
   mechanisms make this non-disableable by the sandbox's user (see
   "Deploying into a sandbox" below).
2. The binary and its classification prompt are baked into the sandbox
   image at a path Landlock already marks `read_only` by default.
3. It classifies the agent's own real session transcript for
   compliance/risk (unauthorized cross-client/cross-banker data access
   attempts) — see
   [`demos/keycloak-oidc/docs/prometheus-scraping.md`](../../demos/keycloak-oidc/docs/prometheus-scraping.md)
   for the full design and its known limitations.

Three hooks, two very different jobs. `SessionStart`/`UserPromptSubmit`
push a lightweight presence heartbeat (`agent_session_started`,
`agent_turn_heartbeat`) with **no LLM call at all** — just "this agent is
alive," cheap enough to fire on every session/turn unconditionally, even
when no classification credential is configured at all. `Stop` is the only
hook that reads the transcript and calls an LLM to classify it.

It's a **one-shot CLI, not a server** — and none of the three hooks add
turn latency either. `Stop` fires after *every* turn, not just at session
end (confirmed from both agents' own docs — see "Multi-agent support"
below), so the visible hook invocation does the minimum possible work for
all three events: read stdin, stage it to a temp file, spawn a detached
copy of itself as `--worker <path>` (stdin/stdout/stderr all
`Stdio::null()`, so the invoking agent — which may wait for the hook's own
stdout to reach EOF — isn't kept waiting for the worker too), and exit.
Confirmed live: the visible hook returns in ~1ms inside a real sandbox; the
actual work (heartbeat push, or classify+push for `Stop`) happens in that
detached worker, entirely after the hook has already returned, and
survives independently of whether the parent hook process already exited.
No `/metrics` endpoint, no long-running process, no polling — just a
short-lived worker per hook invocation instead of a blocking one.

## Multi-agent support

Confirmed live (2026-08-31) that Claude Code and Codex are structurally
similar enough at the hook layer to share one binary, but differ in two
concrete ways this code handles explicitly:

| | Claude Code | Codex |
|---|---|---|
| Managed hook config file | `/etc/claude-code/managed-settings.json` (JSON) | `/etc/codex/requirements.toml` (TOML) |
| Non-overridable lock keys | `allowManagedHooksOnly: true`, `disableAllHooks: false` | `allow_managed_hooks_only = true`, `[features] hooks = true` |
| Transcript path | `~/.claude/projects/<encoded-cwd>/<session-id>.jsonl` | `~/.codex/sessions/YYYY/MM/DD/rollout-<timestamp>-<uuid>.jsonl` |
| Transcript schema | nested — `{"type":"user"\|"assistant","message":{"content":[...]}}` | flat — `{"type":"event_msg","payload":{"type":"user_message"\|"agent_message","message":"..."}}` for text, `{"type":"response_item","payload":{"type":"function_call"\|"function_call_output",...}}` for tool calls |

The binary auto-detects which parser to use from the `transcript_path`
string it receives on stdin (`.codex/sessions/` vs `.claude/projects/`) —
no build-time flag or separate binary needed for this part. The two
*images* still need to differ, because the managed hook **config file**
and its filesystem location are agent-specific and must be baked in at
build time (see below).

Two incidental findings from live-testing Codex's hooks, worth knowing if
you extend this further: a secondary source claimed Codex's non-managed
`hooks.json` has "no wrapper" — live testing showed it needs
`{"hooks": {"Stop": [...]}}`, contradicting that (moot for us, since we
use the managed `requirements.toml` path, not `hooks.json`). And Codex
0.146.0 rejects `wire_api = "chat"` in `config.toml` model providers
("no longer supported") — use `wire_api = "responses"` even against a
plain chat-completions-style backend like DeepSeek.

## Getting the binary

### From GitHub Releases (recommended — no Rust toolchain needed)

```bash
curl -fsSL -o session-auditor \
  https://github.com/alpha-hack-program/openshell-demos/releases/latest/download/session-auditor-linux-x86_64-musl
chmod +x session-auditor
```

Check [the Releases page](https://github.com/alpha-hack-program/openshell-demos/releases)
for the current `session-auditor-v*` tag first — the repo's overall
"latest" release tracks whichever component published most recently, not
necessarily this one, so `releases/latest/download/...` isn't reliable —
use the explicit tag shown there instead.

### From source

Requires Rust 2024 edition (1.85+) and the `x86_64-unknown-linux-musl`
target (`rustup target add x86_64-unknown-linux-musl`).

```bash
cd util/session-auditor
make musl        # builds target/x86_64-unknown-linux-musl/release/session-auditor
```

**Always use the musl target** — a plain `cargo build --release` binary
links against whatever glibc the build machine has, which is typically
newer than the sandbox base images ship. See
[`util/agent-proxy/README.md`](../agent-proxy/README.md#why-static-musl-not-dynamic-linking)
for the full rationale (identical here).

## Deploying into a sandbox

**Only one approach works for this tool — baking the binary into a custom
image at build time.** Unlike `agent-proxy`, uploading the binary into a
stock sandbox at runtime (`sandbox upload`) does not work here, on
purpose: `/usr/local/bin` is `read_only` under OpenShell's *default*
`filesystem_policy` (Landlock), and `sandbox upload` is subject to that
same restriction, not a privileged bypass — the whole point is that
nothing inside a running sandbox, including an admin's own
`sandbox upload`, can replace this binary or its prompt after the fact.

```bash
make image-claude    # or image-codex — copies the musl binary into the
                      # matching demos/keycloak-oidc/images/<agent>-audit/
                      # dir (gitignored there) and runs `podman build -f`
make push-claude      # or push-codex, or plain `push` for both
```

Each image is tagged with the exact tag of the *base* image it's built
from (extracted automatically from the Containerfile's `FROM` line — see
`util/session-auditor/Makefile`), plus `latest` — e.g. `claude-audit:0.3.36`,
`codex-audit:0.0.1-1786355012`. This is deliberate: if the base image
bumps, the derived image's primary tag has to bump to match, so
`claude-audit:latest` silently going stale relative to a newer base image
is visible rather than hidden.

Each Containerfile also bakes in that agent's managed hook config —
[`claude-audit/managed-settings.json`](../../demos/keycloak-oidc/images/claude-audit/managed-settings.json):

```json
{
  "allowManagedHooksOnly": true,
  "disableAllHooks": false,
  "hooks": {
    "Stop": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "/usr/local/bin/session-auditor" }] }
    ],
    "SessionStart": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "/usr/local/bin/session-auditor" }] }
    ],
    "UserPromptSubmit": [
      { "matcher": "*", "hooks": [{ "type": "command", "command": "/usr/local/bin/session-auditor" }] }
    ]
  }
}
```

or [`codex-audit/requirements.toml`](../../demos/keycloak-oidc/images/codex-audit/requirements.toml):

```toml
allow_managed_hooks_only = true

[features]
hooks = true

[hooks]
managed_dir = "/usr/local/bin"

[[hooks.Stop]]
[[hooks.Stop.hooks]]
type = "command"
command = "/usr/local/bin/session-auditor"

[[hooks.SessionStart]]
[[hooks.SessionStart.hooks]]
type = "command"
command = "/usr/local/bin/session-auditor"

[[hooks.UserPromptSubmit]]
[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "/usr/local/bin/session-auditor"
```

`disableAllHooks: false` (Claude Code) and `allow_managed_hooks_only = true`
(Codex) are load-bearing, not decorative: confirmed against each agent's
own official docs that a lower-precedence (user/project/`--settings` or
non-managed hooks source) value cannot override them, so the sandbox owner
cannot turn the hook off from inside their own session.

Then create the sandbox **from that image**, not the stock base image,
and grant network policy for both endpoints the hook needs — scoped to
this binary specifically, so the sandbox owner can't reach either with a
plain `curl` and impersonate the auditor. See
[`demos/keycloak-oidc/providers/session-auditor-anthropic-profile.yaml`](../../demos/keycloak-oidc/providers/session-auditor-anthropic-profile.yaml)
(or [`session-auditor-openai-profile.yaml`](../../demos/keycloak-oidc/providers/session-auditor-openai-profile.yaml) —
see "Classification backend: Anthropic or OpenAI-compatible" below) for a
provider that grants both endpoints (and the classification API key,
network-layer resolved) in one `sandbox provider attach` — confirmed live
that a profile declaring both `endpoints` and `binaries` composes the
network policy automatically, no separate `policy update` call needed.

```bash
openshell sandbox create --name my-sandbox \
  --from quay.io/atarazana/claude-audit:latest \
  --provider session-auditor-anthropic --workspace <ws> -- true
```

That's it — no `sandbox exec`, no `nohup`, no `service expose`. The agent
calls `session-auditor` itself via all three hooks. `SessionStart` and
`UserPromptSubmit` need no configuration at all — no LLM call, no
credential, they just push a heartbeat. `Stop`'s classification call needs
`AUDITOR_LLM_BASE_URL`/`AUDITOR_ANTHROPIC_MODEL` set on the environment the
agent itself runs in (child processes inherit them —
`AUDITOR_ANTHROPIC_API_KEY` comes from the attached provider, and
`OTLP_ENDPOINT` is compiled in, so neither of those needs to be passed):

```bash
openshell sandbox exec -n my-sandbox --workspace <ws> \
  --env "AUDITOR_LLM_BASE_URL=..." --env "AUDITOR_ANTHROPIC_MODEL=..." \
  -- claude -p "..." --permission-mode bypassPermissions
```

These are deliberately `AUDITOR_`-prefixed, not the bare `ANTHROPIC_*`
names Claude Code itself reads — confirmed live that attaching both a
`session-auditor-*` provider and a Claude Code LLM provider (e.g.
`deepseek-claude`) to the same sandbox fails at `sandbox provider attach`
time if both declare the same credential env key ("credential env key
'ANTHROPIC_API_KEY' is provided by both provider 'session-auditor' and
provider 'deepseek-claude'; use provider-specific env names" — the
gateway's own error, which is also where the naming fix came from). This
also means the classification call can point at a completely different
backend/model than the agent's own conversation, on both agents equally —
there's no natural env-var overlap to (accidentally) inherit from on
either.

See
[`demos/keycloak-oidc/docs/prometheus-scraping.md`](../../demos/keycloak-oidc/docs/prometheus-scraping.md#tamper-resistant-metrics)
for the full design and the OpenTelemetry Collector side of this
(see also [`demos/keycloak-oidc/audit-collector/`](../../demos/keycloak-oidc/audit-collector/)).

## Classification backend: Anthropic or OpenAI-compatible

Today's classification call defaults to Anthropic-Messages-API-compatible,
but there's no reason the classifier has to speak the same wire format as
whichever agent (Claude Code or Codex) is actually running in the
sandbox — the two are independent. `--api-style`/`AUDITOR_API_STYLE`
(`anthropic`, the default, or `openai`) selects which one `session-auditor`
speaks for its own classification call, confirmed live for both:

| | `anthropic` (default) | `openai` |
|---|---|---|
| Credential env var | `AUDITOR_ANTHROPIC_API_KEY` | `AUDITOR_OPENAI_API_KEY` |
| Model env var | `AUDITOR_ANTHROPIC_MODEL` | `AUDITOR_OPENAI_MODEL` |
| Default base URL | `https://api.anthropic.com` | `https://api.openai.com` |
| Endpoint + path | `{base_url}/v1/messages`, `x-api-key` header | `{base_url}/v1/chat/completions`, `Authorization: Bearer` header |
| Provider profile | [`session-auditor-anthropic-profile.yaml`](../../demos/keycloak-oidc/providers/session-auditor-anthropic-profile.yaml) | [`session-auditor-openai-profile.yaml`](../../demos/keycloak-oidc/providers/session-auditor-openai-profile.yaml) |

This mirrors the demo's own `byo-claude`/`byo-codex` split — a different
credential env name and auth style per backend wire format — except the
axis here is *which classification backend*, not *which agent*, so either
provider can be attached to either a `claude-audit` or a `codex-audit`
sandbox. `AUDITOR_LLM_BASE_URL` is the one shared override across both
styles (no default baked in — the right default depends on `api_style`,
resolved at runtime). Deliberately plain OpenAI **Chat Completions**, not
the Responses API: classification is a single non-agentic turn with no
tool calls, so Chat Completions is the simplest broadly-compatible choice
(see
[`docs/inference-api-compatibility.md`](../../demos/keycloak-oidc/docs/inference-api-compatibility.md)
for why Responses API matters for the agents themselves, which need
namespace-tool support this call never uses). Both styles parse the same
`Verdict` JSON out of whatever text the model returns — only how the
request/response are wrapped differs.

Confirmed live: `AUDITOR_API_STYLE=openai` against a real OpenAI-compatible
endpoint classified a real Claude Code transcript successfully, alongside
the default `anthropic` style against a real Anthropic-compatible
endpoint — same binary, same image, different `--env` at `sandbox exec`
time.

## Metrics pushed

| Metric | Hook | Type | Attributes | LLM call? |
|---|---|---|---|---|
| `agent_session_started` | `SessionStart` | gauge, always `1` | `session_id`, `agent` (`claude`/`codex`/`unknown`, from which `/etc/claude-code` or `/etc/codex` exists in the image), `workspace`, `sandbox` (both best-effort, see below) | No |
| `agent_turn_heartbeat` | `UserPromptSubmit` | gauge, always `1` | `session_id`, `agent`, `workspace`, `sandbox` | No |
| `session_compliance_risk_score` | `Stop` | gauge, `0`-`3` | `session_id`, `risk_level`, `workspace`, `sandbox` | Yes |

Confirmed live for both agents: `SessionStart`/`UserPromptSubmit` push
successfully even when `AUDITOR_ANTHROPIC_API_KEY`/`AUDITOR_ANTHROPIC_MODEL`
are entirely unset or misconfigured — the two are dispatched before any
`Config::resolve` check, so a missing classification credential never
blocks the heartbeat.

**`workspace`/`sandbox` are best-effort, not a security control.** Both
are read from `/etc/hostname`, which on this OpenShell version is set to
`<workspace>--<sandbox-name>` (confirmed live, 2026-09-02) — split on the
first `--` into the two attributes; if that separator isn't found (e.g. a
future OpenShell version changes the convention), both attributes are
just omitted rather than guessed — never one without the other, since a
half-parsed hostname is more likely to be wrong than informative. This was
chosen over two alternatives that don't work at all on this platform:
`$USER_ACCESS_TOKEN` (this demo's own per-user OIDC credential) is a
network-layer resolve placeholder, never a real token inside the sandbox,
so there's nothing to decode locally; and neither `sandbox upload` nor
`--upload` at creation time can write to a root-owned path like `/etc/`,
so there's no
tamper-resistant channel for admin-supplied *per-instance* data at all
today. Hostname is the one thing that's both readable and confirmed
non-writable by the sandbox's own non-root user (`hostname newname` →
"must be root"; `echo > /etc/hostname` → "Permission denied") — genuinely
harder to spoof than any env var, but it rests on an **undocumented
internal naming convention**, not a stable public API. Treat it the same
way as this repo's `[VERIFY]` tags: re-confirm after an OpenShell version
bump rather than trusting it indefinitely.

## Configuration

| Env var | Default | Description |
|---|---|---|
| `AUDITOR_API_STYLE` | `anthropic` | `anthropic` or `openai` — which wire format the classification call uses. See "Classification backend" above. |
| `AUDITOR_ANTHROPIC_API_KEY` | *(none — `Stop` hook fails soft if unset; `SessionStart`/`UserPromptSubmit` don't need it at all)* | Credential for the classification call when `AUDITOR_API_STYLE=anthropic`. Normally comes from the attached `session-auditor-anthropic` provider, not a manually-passed `--env`. |
| `AUDITOR_ANTHROPIC_MODEL` | *(none — `Stop` hook fails soft if unset)* | Model name when `AUDITOR_API_STYLE=anthropic`. |
| `AUDITOR_OPENAI_API_KEY` | *(none)* | Credential for the classification call when `AUDITOR_API_STYLE=openai`. Normally comes from the attached `session-auditor-openai` provider. |
| `AUDITOR_OPENAI_MODEL` | *(none)* | Model name when `AUDITOR_API_STYLE=openai`. |
| `AUDITOR_LLM_BASE_URL` | `https://api.anthropic.com` or `https://api.openai.com`, depending on `AUDITOR_API_STYLE` | Override for a BYO backend compatible with whichever style is selected. |
| `CLASSIFY_TIMEOUT_SECS` | `20` | Timeout on the classification LLM call, so a hung call can't hang the user's turn. |

None of the above are enforced as clap-required arguments, on purpose: a
`Stop` hook that exits with clap's default code for a missing required
argument (`2`) is read by both Claude Code and Codex as "blocking error —
don't stop," which can hang or corrupt the user's turn. A
missing/misconfigured value here logs (to stderr and to
`/tmp/session-auditor-hook.log`) and exits `0` — the hook must fail soft,
never hard.

**Not configurable at runtime at all, by design (not even via env var):**

- The classification prompt (`prompt.txt`, embedded via `include_str!` at
  compile time). If it were a runtime flag or env var, the sandbox's own
  user could invoke the same binary with a different prompt and get a
  friendlier verdict.
- The OTLP push target (`otlp-endpoint.txt`, same mechanism). If it were
  an env var, the sandbox's own user could redirect or unset it — this
  can't fabricate a false-favorable verdict (network policy still only
  allows the one real collector host), but it could silently suppress the
  audit signal entirely. Baking it in removes that override surface too.

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
`session-auditor-v<version>`, and pushes. GitHub Actions then builds the
musl binary and both sandbox images (`claude-audit`, `codex-audit`),
tagged with each one's base-image tag, `latest`, and this release's own
tag, and publishes a GitHub Release with the binary attached.
