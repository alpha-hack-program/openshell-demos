# Inference API compatibility

This document covers which LLM API formats the agentic coding tools in
this repo require, which providers support them, and how to test
compatibility before running a demo.

## API formats at a glance

| API format | Wire protocol | Who uses it |
|---|---|---|
| **OpenAI Responses API** | `POST /v1/responses` | Codex 0.146.0+ (`wire_api = "responses"`) |
| **OpenAI Chat Completions API** | `POST /v1/chat/completions` | Older Codex, many third-party tools |
| **Anthropic Messages API** | `POST /v1/messages` | Claude Code |

Codex and Claude Code require **different** API formats. You cannot use
one endpoint for both unless your provider exposes both formats (some
providers offer separate OpenAI-compatible and Anthropic-compatible
endpoints under different base URLs).

## Codex: Responses API with namespace tools

Codex 0.146.0+ exclusively uses `wire_api = "responses"` — the OpenAI
Responses API. It sends MCP tools as `"type": "namespace"` tools, a
Responses API extension that groups MCP tools under a named scope (e.g.
`mcp-server-a.evaluate_unpaid_leave_eligibility`).

This means the LLM endpoint must support **both**:
1. The Responses API itself (`/v1/responses`)
2. Namespace-scoped tools (`"type": "namespace"` in the tool definition)

### Provider compatibility

| Endpoint type | Responses API | Namespace tools | Notes |
|---|---|---|---|
| **OpenAI API** | Yes | Yes | Only provider confirmed to support `namespace` tools natively |
| **vLLM (on-cluster)** | Yes (v0.8.0+) | Yes (v0.25.0+) | Older vLLM accepts the Responses API but rejects namespace tools with a 400 error. RHOAI 3.4.x ships vLLM 0.18.0; 0.25+ is upstream only |
| **Most third-party providers** | Yes | **No** | Tested against DeepSeek (Aug 2026) — Responses API works, `namespace` tools rejected (400). Expect similar behaviour from other providers until they add support |

> **`namespace` tools are the key constraint.** Many providers implement
> the Responses API but reject `namespace` tool definitions — they only
> accept `function` tools. Codex model-only calls work fine on these
> providers; only Codex + MCP breaks. Use the test script at the end of
> this doc to verify before deploying.

### Verified combinations

| Codex version | LLM endpoint | MCP server version | Result |
|---|---|---|---|
| 0.146.0 | vLLM 0.27.1 (upstream, on-cluster) | 2.4.1 / 3.2.0 | Pass (full pipeline incl. MCP) |
| 0.146.0 | DeepSeek (external) | 2.4.1 / 3.2.0 | Pass (model-only — namespace tools rejected) |

## Claude Code: Anthropic Messages API

Claude Code uses the Anthropic Messages API format (`/v1/messages`). It
sends MCP tools as standard tool definitions with `input_schema` — no
namespace extension involved.

This means the LLM endpoint must speak the **Anthropic** wire format, not
OpenAI. OpenAI's own API does **not** work (it only speaks its own
format). vLLM v0.18.0+ does support the Messages API on the Python
frontend — see the provider table below.

### Endpoint compatibility

| Endpoint type | Anthropic Messages API | Notes |
|---|---|---|
| **Anthropic API** | Yes | Native |
| **Third-party Anthropic-compatible** | Yes | Some providers expose a separate Anthropic-compatible route (e.g. a `/anthropic` path). Tested against DeepSeek (Aug 2026) |
| **LiteLLM** | Yes (with `/anthropic` route) | Must be explicitly configured to proxy Anthropic format |
| **vLLM (on-cluster)** | Yes (v0.18.0+, Python frontend only) | Rust frontend does not implement Messages API |
| **OpenAI API** | No | Own format only |

> **Dual-endpoint providers.** Some providers expose both an
> OpenAI-compatible and an Anthropic-compatible endpoint under different
> base URLs (same API key). Make sure you configure the right base URL
> for each tool — the OpenAI URL won't work with Claude Code and vice
> versa.

### Verified combinations

| Claude Code version | LLM endpoint | MCP server version | Result |
|---|---|---|---|
| (sandbox default) | DeepSeek Anthropic endpoint (external) | 2.4.1 / 3.2.0 | Pass (incl. MCP tool use) |

## Test script

Quick smoke test to verify your endpoint supports the Responses API with
namespace tools (for Codex). Run from a machine that can reach the
endpoint:

```bash
# Set these to match your provider
OPENAI_BASE_URL="https://your-provider.example.com"
OPENAI_API_KEY="your-api-key"
OPENAI_MODEL="your-model-name"

# 1. Basic Responses API support
echo "=== Testing Responses API ==="
HTTP_CODE=$(curl -sk -o /tmp/responses-test -w "%{http_code}" \
  -X POST "${OPENAI_BASE_URL}/v1/responses" \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "'"${OPENAI_MODEL}"'",
    "input": "Say hello",
    "max_output_tokens": 50
  }')
echo "HTTP: $HTTP_CODE"
if [[ "$HTTP_CODE" != "200" ]]; then
  echo "FAIL: Responses API not supported"
  cat /tmp/responses-test
  exit 1
fi
echo "PASS: Responses API works"

# 2. Namespace tools support
echo ""
echo "=== Testing namespace tools ==="
HTTP_CODE=$(curl -sk -o /tmp/ns-tools-test -w "%{http_code}" \
  -X POST "${OPENAI_BASE_URL}/v1/responses" \
  -H "Authorization: Bearer ${OPENAI_API_KEY}" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "'"${OPENAI_MODEL}"'",
    "input": "What tools do you have?",
    "max_output_tokens": 100,
    "tools": [
      {
        "type": "namespace",
        "name": "test-server",
        "functions": [
          {
            "name": "hello",
            "description": "Says hello",
            "parameters": {"type": "object", "properties": {}, "required": []}
          }
        ]
      }
    ]
  }')
echo "HTTP: $HTTP_CODE"
if [[ "$HTTP_CODE" != "200" ]]; then
  echo "FAIL: Namespace tools not supported (this is the vLLM < 0.25.0 case)"
  cat /tmp/ns-tools-test
  exit 1
fi
echo "PASS: Namespace tools work"
echo ""
echo "Endpoint is compatible with Codex 0.146.0+"
```

For Claude Code, verify the Anthropic Messages API:

```bash
ANTHROPIC_BASE_URL="https://your-provider.example.com/anthropic"
ANTHROPIC_API_KEY="your-api-key"
ANTHROPIC_MODEL="your-model-name"

echo "=== Testing Anthropic Messages API ==="
HTTP_CODE=$(curl -sk -o /tmp/messages-test -w "%{http_code}" \
  -X POST "${ANTHROPIC_BASE_URL}/v1/messages" \
  -H "x-api-key: ${ANTHROPIC_API_KEY}" \
  -H "anthropic-version: 2023-06-01" \
  -H "Content-Type: application/json" \
  -d '{
    "model": "'"${ANTHROPIC_MODEL}"'",
    "max_tokens": 50,
    "messages": [{"role": "user", "content": "Say hello"}]
  }')
echo "HTTP: $HTTP_CODE"
if [[ "$HTTP_CODE" != "200" ]]; then
  echo "FAIL: Anthropic Messages API not supported"
  cat /tmp/messages-test
  exit 1
fi
echo "PASS: Anthropic Messages API works"
echo ""
echo "Endpoint is compatible with Claude Code"
```
