# Anatomy of an OpenShell sandbox policy

The main guide starts granting sandbox policy in
[step 5](../README.md#5-run-the-demo) and leans on it heavily from
[Provision the Claude Code harness](../README.md#provision-the-claude-code-harness)
onward. This document explains what a policy document actually contains,
how the CLI commands that touch it differ, and walks through adding a
one-off endpoint two different ways. Read it once and the rest of the
guide's policy commands should be self-explanatory.

## The policy document

A sandbox's policy is one YAML/JSON document with four top-level sections:

```yaml
version: 1

filesystem_policy:
  include_workdir: true
  read_only: [/usr, /lib, /proc, /dev/urandom, /app, /etc, /var/log]
  read_write: [/sandbox, /tmp, /dev/null]

landlock:
  compatibility: best_effort

process:
  run_as_group: sandbox
  run_as_user: sandbox

network_policies:
  claude_code:
    name: claude-code
    binaries:
      - path: /usr/local/bin/claude
      - path: /usr/bin/node
    endpoints:
      - host: api.anthropic.com
        port: 443
        protocol: rest
        access: full
        enforcement: enforce
        tls: terminate
```

`filesystem_policy`/`landlock`/`process` are the same on every sandbox in
this demo — they control mount points and the Linux user/group the
sandboxed process runs as, not network egress. `network_policies` is a map
of named **groups**. Each group is independent and additive: a request is
allowed if *any* group's `binaries` list includes the calling process and
*any* of that group's `endpoints` matches the target host/port. Fields on
an endpoint:

| Field | Meaning |
|---|---|
| `host`, `port` | What's being reached. |
| `protocol` | `rest` for HTTP(S); other protocols (e.g. `websocket`) exist but aren't used in this demo. |
| `access` | `read-only`, `read-write`, or `full`. |
| `enforcement` | `enforce` actually blocks non-matching traffic; other values exist for audit-only modes. |
| `tls` | `terminate` if OpenShell needs to see inside the TLS session (e.g. to inject credentials or do method/path-scoped `rules`). Omit it for plain pass-through. |
| `rules` | Optional method/path allowlist scoped to this endpoint — see the built-in `github_ssh_over_https` group for an example (`GET /**/info/refs*`, `POST /**/git-upload-pack`). |

A fresh sandbox ships with a bundle of built-in groups —
`claude_code`, `codex`, `copilot`, `cursor`, `github_rest_api`,
`github_ssh_over_https`, `nvidia_inference`, `opencode`, `pypi`, `vscode` —
covering the agent harnesses and dev tools OpenShell knows about
out of the box. You can see them yourself:

```bash
openshell policy get <sandbox> --base -o json
```

## There are three layers, not one

This matters more than it looks: `openshell policy get --full` shows the
*effective* policy, which is actually the base document plus a second,
separate layer contributed by attached providers.

- **The base document** — everything under `network_policies` in the
  stored policy, shown by `policy get --base`. This is what `policy set`
  replaces and what `policy update` merges into.
- **Provider-composed groups** — named `_provider_<name>`, added the
  moment a provider is attached (`sandbox provider attach`, or `--provider`
  at `sandbox create`), from that provider's profile. **Confirmed live:**
  attaching a provider, then running `policy set` with a document that
  never mentioned the provider's endpoint at all, left the
  `_provider_<name>` group in the effective policy afterward — replacing
  the base document does not touch this layer.
- Whether a provider-composed group actually grants binary-scoped access,
  or only exists so OpenShell knows where to inject a credential, depends
  on whether that provider's **profile** declares a `binaries:` list.
  `providers/byo-codex-profile.yaml` does (`/usr/bin/codex`,
  `/usr/local/bin/codex`), so `_provider_byo_codex` is fully
  self-sufficient — confirmed live, Codex could reach `inference.local`
  through that group alone. `providers/byo-claude-profile.yaml` doesn't
  declare any binaries, so its endpoint is available for credential
  matching but the actual authorization to reach it still needs an
  explicit group with `binaries:` — either from `policy update
  --add-endpoint --binary` or, as this demo now does, from a hand-composed
  document applied with `policy set`.

`policy get -o json` (no `--base`/`--full`) shows neither layer, just
version/hash metadata — use `--full` for the effective policy or `--base`
for the stored document alone.

## The commands

| Command | Effect |
|---|---|
| `policy get <sandbox> [--base\|--full] [-o table\|json]` | Read-only. `--base` = stored document, `--full` = document + provider-composed groups, neither = metadata only. |
| `policy update <sandbox> --add-endpoint H:P:ACCESS:PROTO:ENFORCE --binary PATH [...]` | **Merges.** Repeatable flags — multiple `--add-endpoint`/`--binary` pairs in one call are allowed and confirmed to attach correctly to each other. Never touches groups it doesn't mention, including the built-in bundle. |
| `policy set <sandbox> --policy file.yaml` | **Replaces the entire base document.** Anything not in `file.yaml` — including the built-in bundle — is gone from the base layer (provider-composed groups are unaffected, see above). |
| `policy list <sandbox>` | Version history. |
| `policy delete --global` | Removes a gateway-global policy lock. |

The rule of thumb: reach for `policy update` when you're adding one or two
things to whatever's already there. Reach for `policy set` only when you're
prepared to declare *everything* the sandbox needs, because it will not
merge — see the worked example below for what that looks like in practice.

## Worked example: granting a one-off host (a weather API)

Say a sandbox's agent needs read-only `curl` access to
`api.weather.example.com:443` that nothing else grants. Two ways to do it:

**The quick way — `policy update`.** Safe to run against a sandbox that
already has other policy in place; it only adds this one rule:

```bash
openshell policy update my-sandbox \
  --add-endpoint api.weather.example.com:443:read-only:rest:enforce \
  --binary /usr/bin/curl \
  --workspace my-workspace \
  --wait
```

**The full-document way — add a group, then `policy set`.** Only makes
sense if you're already managing this sandbox's policy as a whole document
(as this demo does via the [`policies/`](../policies/) chart). Add a group
to the document:

```yaml
network_policies:
  # ...whatever else the document already has...
  allow_weather_api:
    name: allow_weather_api
    binaries:
      - path: /usr/bin/curl
    endpoints:
      - host: api.weather.example.com
        port: 443
        protocol: rest
        access: read-only
        enforcement: enforce
```

then apply the whole document:

```bash
openshell policy set my-sandbox --policy file.yaml --workspace my-workspace --wait
```

Both produce the identical rule. The difference is blast radius: the first
touches nothing else; the second replaces everything in the base document,
so it's only correct if `file.yaml` also still contains every other group
this sandbox needs (built-ins you want to keep, other MCP servers, the LLM
host, etc.) — see [There are three layers, not one](#there-are-three-layers-not-one)
for what you don't have to re-declare.

## This demo's approach: a Helm chart of policy fragments

Rather than hand-maintain several near-duplicate full policy YAML files
(one per banker, one per agent-harness recipe), this demo composes them
from [`policies/`](../policies/), a small Helm chart with one template.
It's rendered locally and never installed:

```bash
helm template demo-alice-policy policies \
  --set openshellNamespace=keycloak-oidc-demo \
  --set llmHost=api.deepseek.com \
  --set recipe=claude-code \
  --set 'mcpServers={mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance,mcp-compatibility}' \
  > policy.yaml

openshell policy set demo-alice --policy policy.yaml --workspace alice --wait
```

`recipe` (`claude-code` | `codex`) picks which built-in-equivalent
agent-harness group and LLM-host binary get rendered; `mcpServers` is a
plain list, one `allow_<name>` group per entry, each granting
`<name>.<openshellNamespace>.svc.cluster.local:8000` to both `curl` (for
[Annex B](../README.md#b-raw-mcp-protocol-calls-curl-for-scriptingci)'s
raw-protocol walkthrough) and the recipe's own binary. `llmModel` is
deliberately **not** a value here — a network policy only ever gates on
host/port/binary, and the model name never appears in one; it's config
passed to the agent via `--env` at exec time instead. See
[`policies/values.yaml`](../policies/values.yaml) for the full schema and
[`policies/templates/policy.yaml`](../policies/templates/policy.yaml) for
the template itself.

> **This is a demo convenience, not a policy-management best practice.**
> It exists to turn several near-identical, hard-to-diff `policy update`
> loops into one readable, parameterized command. It is **not**
> schema-validated against OpenShell's actual policy format beyond
> whatever `policy set` itself rejects at apply time — the schema is
> undocumented and could change across `openshell` versions, and this
> chart would silently drift if it did. Helm is being repurposed here
> purely as a local text-templating engine with conditionals and loops;
> nothing in `policies/` is ever installed to a cluster, and there is no
> Kubernetes resource, release, or values validation happening beyond what
> `helm template` does for any chart. A real production policy pipeline
> would look completely different — likely generated and validated by
> whatever tooling OpenShell itself ships for this (if any), or managed
> through the same GitOps/config-management system as the rest of your
> fleet, with actual schema validation and drift detection. Don't copy
> this pattern into a non-demo context without that context in mind.
