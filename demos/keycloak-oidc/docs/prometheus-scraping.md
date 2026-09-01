# Compliance/risk metrics from an agent sandbox

How `session-auditor` gets a tamper-resistant compliance/risk metric out
of a Claude Code or Codex sandbox and into Prometheus. One binary, shared
across both agents — see
[`util/session-auditor/README.md`](../../../util/session-auditor/README.md#multi-agent-support)
for exactly what's shared versus agent-specific.

## Why not just scrape the sandbox directly

`openshell service expose` creates an HTTPS endpoint routed entirely by
HTTP **Host header** through the gateway's single shared Route — multiple
sandboxes' exposed services share one hostname, disambiguated only by
`Host:`. A client that can't set a custom Host header (a Prometheus
`ServiceMonitor` — `ServiceMonitor.spec.endpoints` has no `headers` field
on prometheus-operator versions that predate it) can therefore only ever
reach the gateway's *default* vhost, never a specific sandbox. This isn't
a policy restriction that could be loosened — it's structural: the
process a sandbox runs lives in its own nested network namespace,
connected to the outer pod only by a single veth pair to the supervisor's
own proxy (see [Sandbox network isolation](sandbox-network-isolation.md)).
It has no presence on the real pod network at all, so there's no pod-IP
route to it from any other pod, `NetworkPolicy` aside.

A Host-rewrite proxy (the pattern [`garak-envoy`](evalhub-redteam.md) uses
for the unrelated EvalHub/Garak red-team demo) can solve this for a
*scraped* metric. `session-auditor` doesn't need it: it pushes instead of
being scraped, so the sandbox only ever needs outbound network access —
no inbound routing problem to solve at all.

## Why a plain writable file/metric isn't enough here

For a compliance/risk score — mirroring [Scene 4 — Bob
overreaches](../README.md#scene-4--bob-overreaches) — the value matters
*because* the sandbox's user (or an agent acting on their behalf) has an
incentive to fake it. A plain writable file isn't enough, and neither is a
`read_only` Landlock rule on its own: OpenShell's `filesystem_policy` has
no per-binary scoping, so there's no way to let one binary write a path
while denying another. The sandbox owner and any agent running inside are,
at this layer, indistinguishable.

[`util/session-auditor`](../../../util/session-auditor/) resolves this by
combining three independent protections and pushing its result out:

**It's invoked by the agent itself, not by anything the sandbox owner
runs.** It's registered as `SessionStart`, `UserPromptSubmit`, and `Stop`
hooks via each agent's own managed, non-user-overridable config
mechanism — `/etc/claude-code/managed-settings.json` for Claude Code,
`/etc/codex/requirements.toml` for Codex (both `read_only` under
OpenShell's *default* `filesystem_policy` — the same protection
`/usr/local/bin` gets, free). Confirmed live against both agents' own docs
that neither's lock keys (`disableAllHooks: false` for Claude Code,
`allow_managed_hooks_only = true` for Codex) can be overridden by a
user/project-level value, so the sandbox owner cannot turn any of the three
hooks off from inside their own session, on either agent.

**The binary and its prompt are baked into the image, not uploaded.** Same
reasoning as the hook config: `/usr/local/bin` is `read_only` by default,
and `openshell sandbox upload` is subject to that same Landlock
enforcement as a direct write — confirmed live, not assumed. The
classification prompt (`util/session-auditor/prompt.txt`) is compiled
into the binary via `include_str!`, not read from a runtime flag or env
var. Updating either means rebuilding the image and recreating the
sandbox — there is no live-patch path, by design.

**Three hooks, two very different jobs — and none of them add turn
latency.** `SessionStart` fires once when a session starts (or resumes),
`UserPromptSubmit` fires once per turn, and `Stop` fires after *every*
agent response, not just at session end — confirmed from both agents' own
docs (Claude Code's hooks table lists `Stop` and `SessionEnd` as separate
events, and its `Stop` payload's `stop_hook_active` field only makes sense
if `Stop` can recur within a session; Codex's docs explicitly categorize
`Stop` as "during a turn," `SessionEnd` as "when the main thread ends").
`SessionStart`/`UserPromptSubmit` are deliberately just a heartbeat — no
transcript read, no LLM call, no classification credential needed at
all — pushing `agent_session_started`/`agent_turn_heartbeat` (gauge, always
`1`) with `session_id`/`agent` attributes, so "is this agent actually
doing anything" is answerable even when the compliance classifier is
misconfigured or the LLM backend is down. `Stop` is the only hook that
reads the transcript and calls an LLM.

Since an audit hook should observe, not gate, `session-auditor`'s visible
hook entry point does the minimum possible work for all three events: read
stdin, stage it to a temp file, spawn a detached re-exec of itself as
`--worker <path>` with `Stdio::null()` on all three standard streams (so
the invoking agent, which may wait for the hook's own stdout to reach EOF,
isn't kept waiting for the worker too), and exit. The worker reads
`hook_event_name` from the staged event and dispatches: the two heartbeats
push immediately with no further I/O; `Stop` does the classify+push work.
Confirmed live for both agents, all three hooks: the visible hook returns
in ~1ms inside a real sandbox (measured directly), the worker still
completes and pushes successfully after the agent has moved on, and it
survives independently of whether the parent hook process already
exited — nothing in either agent's own process-tree cleanup kills it.

Every error path in the worker logs (to stderr and to
`/tmp/session-auditor-hook.log`) rather than propagating — a missing
config value, an unreadable transcript, or a failed LLM/push call has no
one left to report to by the time it happens, since the visible hook
already returned. The visible hook's own error paths (stdin read, staging
the temp file, spawning the worker) also exit `0` unconditionally: clap's
default missing-required-argument exit code (`2`) is read by both agents
as "blocking error — don't stop," so `session-auditor` treats every
required setting as optional and fails soft instead of relying on clap's
own validation.

**The push target is the Red Hat build of OpenTelemetry**, already
installed on this cluster as a supported Operator (`OpenTelemetryCollector`
CRD) — deployed via [`../audit-collector/`](../audit-collector/), a small
chart wrapping the CR plus a `ServiceMonitor`. `session-auditor` `curl`s a
plain OTLP/HTTP JSON payload to the collector's `/v1/metrics` endpoint at
a fixed short DNS name (`audit-collector`, compiled into the binary — see
`util/session-auditor/otlp-endpoint.txt` — not read from an env var at
all, so it can't be redirected or unset by the sandbox owner either); the
collector's own Prometheus exporter re-exposes it, scraped as normal via
the `ServiceMonitor` into `openshift-user-workload-monitoring`. The
sandbox only ever needs *egress* — to the LLM host and to the collector's
in-cluster Service — both scoped via `network_policies`' per-binary
scoping (`--binary /usr/local/bin/session-auditor`) so the sandbox owner
can't reach either endpoint with a plain `curl` and impersonate the
auditor. In practice this grant comes from attaching
[`demos/keycloak-oidc/providers/session-auditor-anthropic-profile.yaml`](../providers/session-auditor-anthropic-profile.yaml)
(or its [`-openai-`](../providers/session-auditor-openai-profile.yaml)
sibling — the classification call can target either an
Anthropic-Messages-API-compatible or an OpenAI-Chat-Completions-compatible
backend, independent of which agent runs in the sandbox; see
[`util/session-auditor/README.md`](../../../util/session-auditor/README.md#classification-backend-anthropic-or-openai-compatible))
to the sandbox — confirmed live that a provider profile declaring both
`endpoints` and `binaries` composes the network policy automatically on
`sandbox provider attach`, no separate `policy update` call needed; it
also injects the classification API key as a network-layer-resolved
placeholder, so the sandbox owner never sees the real secret.

```
Agent (SessionStart once, UserPromptSubmit per turn, Stop per turn)
   │ stdin: {session_id, hook_event_name, transcript_path (Stop only), ...}
   ▼
session-auditor (visible hook — stages stdin, spawns worker, exits: ~1ms)
   │ spawn --worker <path>, Stdio::null() on all three streams
   ▼
session-auditor --worker (detached, runs after the hook already returned)
   │ dispatches on hook_event_name:
   │   SessionStart/UserPromptSubmit → curl → audit-collector:4318 (heartbeat, no LLM call)
   │   Stop                          → curl → LLM (classify), then curl → audit-collector:4318
   │ both curls scoped by network_policy: --binary session-auditor
   ▼
OpenTelemetryCollector (receivers.otlp → exporters.prometheus :8889)
   ▼
ServiceMonitor → openshift-user-workload-monitoring Prometheus
```

`session-auditor` pushes:

```
agent_session_started{session_id="...", agent="claude"|"codex", workspace="...", sandbox="..."}   # gauge, always 1, from SessionStart
agent_turn_heartbeat{session_id="...", agent="claude"|"codex", workspace="...", sandbox="..."}    # gauge, always 1, from UserPromptSubmit
session_compliance_risk_score{session_id="...", risk_level="...", workspace="...", sandbox="..."} # gauge, 0-3, from Stop
```

`workspace`/`sandbox` are best-effort attribution, not a security
control — see
[`util/session-auditor/README.md`](../../../util/session-auditor/README.md#metrics-pushed)
for why (both read from `/etc/hostname`'s `<workspace>--<sandbox-name>`
convention, chosen after two other approaches — decoding
`$USER_ACCESS_TOKEN` locally, and writing an admin-supplied label via
`--upload` to a root-owned path — were confirmed live to not work at all
on this platform).

Confirmed live for **both** agents, each with a real turn, real
transcript (including a real tool call for Codex), a real LLM
classification call, and all three metrics queryable via Prometheus's
own API with the real `session_id`, `workspace`, and `sandbox` attached —
no manual `sandbox exec`/`nohup` step, for either agent. Also confirmed
live: the two
heartbeat metrics land even for a session where the `Stop` classification
call never succeeds (observed directly — a session that failed
authentication before `session-auditor`'s `Config` was even correctly
wired still produced `agent_session_started`/`agent_turn_heartbeat`), since
neither heartbeat handler touches `Config::resolve` at all. Also confirmed
live: `AUDITOR_API_STYLE=openai` against a real OpenAI-compatible endpoint
classified a real transcript successfully, alongside the default
`anthropic` style against a real Anthropic-compatible endpoint — same
binary and image, selected purely by `--env` at `sandbox exec` time. Also
confirmed live along the way: the
`session-auditor-*` provider's credential (network-layer resolved) and
auto-composed network policy work identically for both `claude-audit` and
`codex-audit` sandboxes, since the binary path
(`/usr/local/bin/session-auditor`) is the same in both images.

Two real bugs found and fixed live, worth calling out. `curl` does not
exit non-zero on HTTP error responses (e.g. a `403` from the enforcing
proxy) unless `-f`/`--fail` is passed — both the classification and
OTLP-push `curl` invocations were missing it, meaning a policy-denied or
server-error response would have been silently treated as success. Fixed
by adding `-f` to both calls.

Separately, when adding the heartbeat hooks, attaching both
`session-auditor` and a Claude Code LLM provider (`deepseek-claude`) to
the same sandbox failed at `sandbox provider attach` time: both providers
declared the classification/conversation credential under the same env
key, `ANTHROPIC_API_KEY`, and the gateway rejects two providers claiming
the same credential env key on one sandbox ("use provider-specific env
names"). Fixed by renaming `session-auditor`'s env vars to
`AUDITOR_ANTHROPIC_API_KEY`/`AUDITOR_ANTHROPIC_MODEL`/`AUDITOR_LLM_BASE_URL`
(and their `AUDITOR_OPENAI_*` siblings once the second wire format was
added) — see
[`util/session-auditor/README.md`](../../../util/session-auditor/README.md#configuration).

`workspace`/`sandbox` attribution went through two rejected designs before
landing on hostname-parsing, each confirmed live to be a dead end rather
than assumed: decoding `$USER_ACCESS_TOKEN` locally doesn't work because
it's a network-layer resolve placeholder, not a real JWT, inside the
sandbox — the real credential is only ever substituted at the egress proxy
on an outbound request, never materializes in the sandbox's own process
environment; and neither `sandbox upload` nor `--upload` at `sandbox
create` time can write to a root-owned path like `/etc/` (confirmed with
`mkdir: cannot create directory '/etc/session-auditor': Permission
denied`), so there's no tamper-resistant channel for admin-supplied
*per-instance* data today, the way there is for fleet-wide constants
baked into the shared image. `/etc/hostname` (platform-set to
`<workspace>--<sandbox-name>`, confirmed non-writable by the sandbox's own
non-root user) was the only remaining option that's both readable and not
spoofable — and since it already contains both halves, `session-auditor`
splits out both `workspace` and `sandbox` from the one read, together
(never one without the other, in case the format ever splits unevenly).

## Known limitations

- `workspace`/`sandbox` rest on an **undocumented internal naming
  convention** (`<workspace>--<sandbox-name>` in `/etc/hostname`), not a
  stable public API — confirmed live on this OpenShell version only. A
  future OpenShell release could change the format silently;
  `session-auditor` fails soft (omits both attributes) if the `--`
  separator isn't found, but that's a correctness safeguard, not a
  substitute for re-verifying this after a version bump, the same as any
  other `[VERIFY]` item in this repo.
- The two heartbeat metrics (`agent_session_started`, `agent_turn_heartbeat`)
  only prove the agent process ran a hook — they say nothing about what
  happened *during* the turn. They're a liveness/usage signal ("is this
  agent doing anything at all"), not a compliance signal; that's still
  `session_compliance_risk_score`'s job alone.
- Firing per turn (not per session) means one classification LLM call per
  turn, not per session — cost scales with conversation length. Fixing
  the *latency* problem (background worker) doesn't fix the *cost* one;
  a long interactive session still triggers many classification calls.
  Each call re-examines the transcript up to the last ~12,000 characters,
  so later turns in a long session re-classify overlapping content from
  earlier turns rather than just the delta since the last `Stop`.
- This protects the *auditor, its prompt, and the hook registration
  itself*, not the transcript the auditor reads. `~/.claude/projects/`
  lives under the sandbox's normal writable working directory — a user
  editing their own conversation history before the `Stop` hook fires is
  a real attack surface this design doesn't close.
- The classification prompt is a first draft, not hardened against
  transcripts specifically crafted to fool the classifying LLM itself
  (prompt injection against the auditor's own call) — a separate, harder
  problem.
- `max_tokens` for the classification call needed to be raised from an
  initial `300` to `1024` after a live test with a long, tool-heavy
  transcript: a reasoning model (DeepSeek) spent its whole token budget on
  an internal "thinking" block and never reached the actual JSON answer.
  Watch for the same failure mode if switching models/prompts.
- Only exercised with a benign transcript for Codex live (a plain reply
  and a trivial `echo` tool call) — Codex's classification of an actual
  Scene-4-style overreach attempt hasn't been tested live the way Claude
  Code's was with synthetic transcripts earlier; the parser itself was
  validated against a real captured Codex transcript, just not yet a
  transcript containing a real denial.
