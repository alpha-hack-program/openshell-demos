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
supply an API key. No model serving to manage. Any OpenAI- or
Anthropic-compatible provider works (DeepSeek, Together, Fireworks, etc.)
— the constraint is which wire format and tool types the provider supports.

| Endpoint type | Codex (Responses API) | Claude Code (Messages API) | What to check |
|---|---|---|---|
| OpenAI Responses API + `namespace` tools | **Works** (incl. MCP) | N/A | Only OpenAI's own API supports `namespace` tools today. Most third-party Responses API implementations reject them (400) |
| OpenAI Responses API (`function` tools only) | **Model only** | N/A | Codex model-only calls work. Codex + MCP fails — Codex sends MCP tools as `namespace`, which the provider rejects. Tested against DeepSeek (Aug 2026) |
| Anthropic Messages API | N/A | **Works** (incl. MCP) | Claude Code sends tools in Anthropic format — no `namespace` issue. Tested against DeepSeek's `/anthropic` route (Aug 2026) |

> **`namespace` tools are an OpenAI-only feature today.** Most third-party
> providers that expose a Responses API accept `function` tools but reject
> `namespace`. If you need Codex + MCP with an external provider, verify
> `namespace` tool support before deploying — use the test script at the
> end of this doc.

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
> (e.g. `vllm/vllm-openai`).

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
(e.g. OpenAI's own API), Codex 0.146.0 + MCP works out of the box. Most
third-party providers (DeepSeek, etc.) do **not** support `namespace` tools
yet — Codex model-only calls work, but MCP tool calls fail with a 400.

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
| Messages API at `/v1/messages` | Works on vLLM 0.18.0+ (on-cluster, Python frontend only) and external Anthropic-compatible endpoints |
| Tool calling via Messages API | vLLM translates Anthropic tool format to OpenAI internally — the model itself does not need native Anthropic tool support |
| MCP client | Built-in (not rmcp) — no `namespace` tool issue |

> **vLLM caveat:** the `/v1/messages` endpoint is only available on vLLM's
> Python frontend. The Rust frontend (`VLLM_USE_RUST_FRONTEND=1`) does not
> implement the Messages API.

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

Server image tags deployed by the Helm chart (`mcp-servers/values.yaml`):
- `compatibility-engine-mcp-rs:3.2.0` — fixed (`statefulMode: false`)
- `eligibility-engine-mcp-rs:2.4.1` — fixed (`statefulMode: false`)

## Decision tree

```
What is your LLM endpoint?
├── External hosted API
│   ├── Supports Responses API + namespace tools? (currently OpenAI only)
│   │   └── Yes → Codex + MCP works
│   ├── Supports Responses API (function tools only)?
│   │   └── Yes → Codex model-only works (no MCP)
│   └── Supports Anthropic Messages API?
│       └── Yes → Claude Code + MCP works
│
└── On-cluster llm-d (vLLM)
    ├── RHOAI 3.4.x (vLLM 0.18.0)
    │   ├── Claude Code → Works (Messages API, Python frontend)
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
