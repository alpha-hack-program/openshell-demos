# Inference API compatibility for agentic coding tools

## Context

The Codex recipe in this demo routes model calls through OpenShell's
`inference.local` privacy router to a BYO LLM endpoint. The MCP tool calls
go directly from the sandbox to the in-cluster MCP servers. For the full
pipeline to work, both the **model endpoint** (inference API) and the
**MCP transport** (Streamable HTTP) must be compatible with the agentic
coding tool (Codex or Claude Code).

## Findings (August 2026)

### API support matrix — vLLM 0.18.0 (RHAII 3.4.x)

Tested against three vLLM 0.18.0 endpoints behind llm-d:

| API | Endpoint | Status | Notes |
|---|---|---|---|
| Chat Completions | `/v1/chat/completions` | **Works** | Standard OpenAI format |
| Responses API (no tools) | `/v1/responses` | **Works** | Text generation only |
| Responses API + `function` tools | `/v1/responses` | **Works** | Model generates structured `function_call` output |
| Responses API + `namespace` tools | `/v1/responses` | **Fails (400)** | vLLM rejects `"type": "namespace"` — needs >= 0.25.0 |
| Messages API (Anthropic) | `/v1/messages` | **Works** | Anthropic Messages format with tool use |

### Codex compatibility

| Codex version | `wire_api` | rmcp version | MCP Streamable HTTP | `namespace` tools required | Net result |
|---|---|---|---|---|---|
| 0.117.0 (chart 0.0.97/0.0.101) | `chat` or `responses` | 0.15.0 | **Broken** — `JsonRpcMessage` parse error | No (`chat` works) | MCP tools unusable |
| 0.146.0 (custom image) | `responses` only | newer | **Works** | **Yes** — sends MCP tools as `namespace` type | Blocked by vLLM < 0.25.0 |

> **Why `namespace` instead of `function`?** Codex 0.146.0 dropped
> `wire_api = "chat"` and only supports the OpenAI Responses API. When
> Codex registers an MCP server, it presents its tools to the model as a
> `"type": "namespace"` tool — an OpenAI Responses API concept that groups
> related tools under a named scope (e.g. `compatibility/calc_tax`). This
> is purely a **tool definition format** sent to the model endpoint; MCP
> tool calls still execute locally inside the sandbox, not server-side.
> The model sees the namespace, decides to call a tool within it, and Codex
> routes the call to the MCP server over Streamable HTTP. The problem is
> that `namespace` is a newer Responses API extension — vLLM < 0.25.0
> doesn't recognise it and rejects the entire request with a 400. A
> `function`-typed tool definition would work fine on older vLLM, but Codex
> doesn't offer that option — it unconditionally wraps MCP tools in
> `namespace`. This means Codex 0.146.0 + MCP requires both a newer rmcp
> (for the transport) **and** a Responses API endpoint that accepts
> `namespace` tools (for the model calls).

### Claude Code compatibility

Claude Code uses the Anthropic Messages API (`/v1/messages`), not the
OpenAI Responses API. It has its own MCP client (not rmcp). If the endpoint
supports the Messages API, Claude Code can be used as an alternative to
Codex for the agentic recipe.

| Requirement | Status |
|---|---|
| Messages API at `/v1/messages` | Works on vLLM 0.18.0+ |
| Tool calling via Messages API | Requires model support |
| MCP client | Built-in (not rmcp) |

### vLLM version requirements

| Feature | Minimum vLLM | RHOAI image |
|---|---|---|
| Responses API (basic) | 0.10.0 | Available |
| Responses API + `function` tools | 0.18.0 | Available (current) |
| Responses API + `namespace` tools | **0.25.0** | `rhoai/odh-vllm-cuda-rhel9:v2.25.9` (July 2026) |

### MCP server compatibility

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
Is your LLM endpoint OpenAI Responses API compatible?
├── Yes
│   ├── Supports `namespace` tools (vLLM >= 0.25.0)?
│   │   ├── Yes → Use Codex 0.146.0 + MCP servers (full pipeline)
│   │   └── No  → Use Codex 0.146.0 for model only (no MCP tools)
│   └── Supports `function` tools only?
│       └── MCP tools won't work with Codex 0.146.0
└── No
    └── Supports Anthropic Messages API?
        ├── Yes → Use Claude Code recipe (Messages API + built-in MCP)
        └── No  → Not compatible with either recipe
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
