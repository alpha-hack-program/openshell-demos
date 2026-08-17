# Inference API compatibility for agentic coding tools

## Context

The Codex and Claude Code recipes in this demo route model calls through
OpenShell's `inference.local` privacy router to a BYO LLM endpoint. MCP
tool calls go directly from the sandbox to the in-cluster MCP servers. For
the full pipeline to work, both the **model endpoint** (inference API) and
the **MCP transport** (Streamable HTTP) must be compatible with the agentic
coding tool.

The LLM endpoint can be either **external** (a hosted API outside
OpenShift) or **on-cluster** (a model served via llm-d / vLLM inside the
same OpenShift cluster). The API compatibility requirements are different
for each case.

## External endpoints (hosted APIs)

External providers expose a managed API — you point OpenShell at a URL and
supply an API key. No model serving to manage.

| Provider | Codex (Responses API) | Claude Code (Messages API) | Notes |
|---|---|---|---|
| OpenAI API | **Works** | N/A | Native Responses API, including `namespace` tools |
| DeepSeek (OpenAI endpoint) | **Works** | N/A | `https://api.deepseek.com`, model `deepseek-v4-flash` |
| DeepSeek (Anthropic endpoint) | N/A | **Works** | `https://api.deepseek.com/anthropic`, model `deepseek-chat` |
| Any Anthropic Messages-compatible API | N/A | **Works** | e.g. LiteLLM with `/anthropic` route |

With external endpoints, API compatibility is a function of the provider —
if they support the wire format your tool needs, it works. No vLLM version
to worry about.

## On-cluster models via llm-d (vLLM)

When you serve a model on OpenShift using llm-d, vLLM is the inference
engine. The vLLM version determines which APIs and tool-definition formats
are available.

### API support matrix — vLLM versions

Tested against llm-d endpoints on OpenShift:

| API | Endpoint | vLLM 0.18.0 (RHOAI 3.4.x) | vLLM 0.25.0+ (upstream only) |
|---|---|---|---|
| Chat Completions | `/v1/chat/completions` | **Works** | **Works** |
| Responses API (no tools) | `/v1/responses` | **Works** | **Works** |
| Responses API + `function` tools | `/v1/responses` | **Works** | **Works** |
| Responses API + `namespace` tools | `/v1/responses` | **Fails (400)** | **Works** |
| Messages API (Anthropic) | `/v1/messages` | **Works** | **Works** |

### vLLM version availability

| vLLM version | Where available | Notes |
|---|---|---|
| 0.18.0 | **RHOAI 3.4.x** (current supported release) | No `namespace` tools support |
| 0.25.0+ | **Upstream only** — not yet shipped in any RHOAI release | Required for Codex + MCP (namespace tools) |

> **No RHOAI image for vLLM 0.25.** As of August 2026, RHOAI ships
> vLLM 0.18.0. vLLM 0.25+ is only available via upstream container images
> (e.g. `vllm/vllm-openai`). Do not confuse old RHOAI image tags (like
> `rhoai/odh-vllm-cuda-rhel9:v2.25.x`, which is a pre-3.x RHOAI version
> number) with the vLLM engine version — those are unrelated version
> schemes.

## Codex compatibility

Codex uses the **OpenAI Responses API** (`wire_api = "responses"`). When
Codex registers an MCP server, it presents its tools to the model as
`"type": "namespace"` tools — a Responses API extension that groups related
tools under a named scope (e.g. `compatibility/calc_tax`).

| Codex version | `wire_api` | rmcp version | MCP Streamable HTTP | `namespace` tools required | Net result |
|---|---|---|---|---|---|
| 0.117.0 (chart 0.0.97/0.0.101) | `chat` or `responses` | 0.15.0 | **Broken** — `JsonRpcMessage` parse error | No (`chat` works) | MCP tools unusable |
| 0.146.0 (custom image) | `responses` only | newer | **Works** | **Yes** — sends MCP tools as `namespace` type | Needs endpoint with `namespace` support |

### Codex + external endpoint

If your external endpoint supports the Responses API with `namespace` tools
(e.g. OpenAI API, DeepSeek), Codex 0.146.0 works out of the box — no
version constraints.

### Codex + on-cluster llm-d

Codex 0.146.0 with MCP requires `namespace` tool support, which means
**vLLM >= 0.25.0**. Since RHOAI does not yet ship this version, your
options are:

1. **Use an upstream vLLM image** (e.g. `vllm/vllm-openai:v0.27.1`) via a
   custom `InferenceService` / `ServingRuntime` — tested and works.
2. **Use Codex without MCP** — model-only calls work on vLLM 0.18.0
   (RHOAI 3.4.x) since plain `function` tools are fine. Only `namespace`
   tools (MCP integration) require 0.25.0+.
3. **Wait for RHOAI to ship vLLM 0.25+** — not yet on any published roadmap.

> **Why `namespace` instead of `function`?** Codex 0.146.0 dropped
> `wire_api = "chat"` and only supports the Responses API. When Codex
> registers an MCP server, it wraps its tools as `"type": "namespace"` — an
> OpenAI concept that groups MCP tools under a named scope. This is purely a
> **tool definition format** sent to the model endpoint; MCP tool calls
> still execute locally inside the sandbox. The model sees the namespace,
> decides to call a tool, and Codex routes the call to the MCP server over
> Streamable HTTP. vLLM < 0.25.0 doesn't recognise `namespace` and rejects
> the request with a 400. A `function`-typed definition would work, but
> Codex doesn't offer that option.

## Claude Code compatibility

Claude Code uses the **Anthropic Messages API** (`/v1/messages`), not the
OpenAI Responses API. It has its own MCP client (not rmcp).

| Requirement | Status |
|---|---|
| Messages API at `/v1/messages` | Works on vLLM 0.18.0+ (on-cluster) and external Anthropic-compatible endpoints |
| Tool calling via Messages API | Requires model support for Anthropic tool-use format |
| MCP client | Built-in (not rmcp) — no `namespace` tool issue |

### Claude Code + external endpoint

Any Anthropic Messages API-compatible endpoint works (e.g. DeepSeek's
`/anthropic` route, a LiteLLM proxy). No vLLM involved.

### Claude Code + on-cluster llm-d

vLLM 0.18.0 (RHOAI 3.4.x) already supports the Messages API, so Claude
Code works on-cluster without needing an upstream vLLM version. The
`namespace` tool limitation does not apply — Claude Code sends tools in
Anthropic's own format, which vLLM handles natively.

## MCP server compatibility

The MCP servers (eligibility-engine-mcp-rs, compatibility-engine-mcp-rs)
use Streamable HTTP transport. Two issues were identified and fixed:

1. **SSE-wrapped responses** — rmcp 0.15.0 (Codex 0.117.0) cannot parse
   SSE-wrapped JSON-RPC responses. Fixed server-side by adding
   `.with_stateful_mode(false)` before `.with_json_response(true)` in
   `streamable_http_config()`. The server now returns `Content-Type:
   application/json` with plain JSON bodies.

2. **Protocol version mismatch** — servers advertise `protocolVersion:
   2025-06-18` in their initialize response. Older rmcp versions may not
   recognize this version. Fixed by upgrading to Codex 0.146.0 (newer rmcp).

Server image tags with the fix:
- `compatibility-engine-mcp-rs:3.1.5` — fixed
- `eligibility-engine-mcp-rs:2.0.2` — **not yet fixed** (apply same change)

## Decision tree

```
What is your LLM endpoint?
├── External hosted API (OpenAI, DeepSeek, etc.)
│   ├── Supports OpenAI Responses API + namespace tools?
│   │   └── Yes → Codex 0.146.0 + MCP (full pipeline)
│   ├── Supports Anthropic Messages API?
│   │   └── Yes → Claude Code recipe (Messages API + built-in MCP)
│   └── Supports both?
│       └── Either tool works — pick based on preference
│
└── On-cluster llm-d (vLLM)
    ├── RHOAI 3.4.x (vLLM 0.18.0)
    │   ├── Claude Code → Works (Messages API supported)
    │   ├── Codex model-only (no MCP) → Works (function tools OK)
    │   └── Codex + MCP → Blocked (namespace tools need vLLM 0.25+)
    │
    └── Upstream vLLM >= 0.25.0
        ├── Claude Code → Works
        └── Codex + MCP → Works (namespace tools supported)
```

## Testing

Use `scripts/09-test-inference-api.sh` to test an endpoint's API
compatibility:

```bash
./scripts/09-test-inference-api.sh \
  --url https://your-endpoint/v1 \
  --model your-model-name \
  --api-key your-api-key
```

The script tests all five API combinations and outputs a markdown table.
