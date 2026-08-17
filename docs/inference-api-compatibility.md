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
one endpoint for both unless the provider exposes both formats (e.g.
DeepSeek serves OpenAI at `https://api.deepseek.com` and Anthropic at
`https://api.deepseek.com/anthropic`).

## Codex: Responses API with namespace tools

Codex 0.146.0+ exclusively uses `wire_api = "responses"` — the OpenAI
Responses API. It sends MCP tools as `"type": "namespace"` tools, a
Responses API extension that groups MCP tools under a named scope (e.g.
`mcp-server-a.evaluate_unpaid_leave_eligibility`).

This means the LLM endpoint must support **both**:
1. The Responses API itself (`/v1/responses`)
2. Namespace-scoped tools (`"type": "namespace"` in the tool definition)

### Provider compatibility

| Provider | Responses API | Namespace tools | Minimum version | Notes |
|---|---|---|---|---|
| **OpenAI** | Yes | Yes | — | Native support |
| **vLLM** | Yes (v0.8.0+) | Yes (v0.25.0+) | **v0.25.0** | Older vLLM accepts the Responses API but rejects namespace tools with a 400 error |
| **DeepSeek** | Yes | Yes | — | Use `https://api.deepseek.com`, model `deepseek-v4-flash` |
| **Ollama** | No | No | — | Chat Completions only |
| **LiteLLM** | Partial | No | — | Proxies Chat Completions; does not translate namespace tools |

**The vLLM version boundary is the most common gotcha.** vLLM v0.8.0
added the Responses API, so Codex can connect and start a session — but
MCP tool calls fail with a 400 because vLLM didn't understand namespace
tools until v0.25.0. The error is not obvious: Codex reports a generic
tool-call failure, not "namespace tools unsupported."

### Verified combinations

| Codex version | LLM provider | MCP server version | Result |
|---|---|---|---|
| 0.146.0 | vLLM 0.27.1 | 3.1.5 | Pass |
| 0.146.0 | DeepSeek (`deepseek-v4-flash`) | 2.0.2 / 3.1.1 | Pass |

## Claude Code: Anthropic Messages API

Claude Code uses the Anthropic Messages API format (`/v1/messages`). It
sends MCP tools as standard tool definitions with `input_schema` — no
namespace extension involved.

This means the LLM endpoint must speak the **Anthropic** wire format, not
OpenAI. Standard OpenAI-compatible endpoints (vLLM, OpenAI's own API)
will **not** work.

### Provider compatibility

| Provider | Anthropic Messages API | Notes |
|---|---|---|
| **Anthropic** | Yes | Native |
| **DeepSeek** | Yes | Use `https://api.deepseek.com/anthropic`, model `deepseek-chat` (different URL and model name from the OpenAI endpoint) |
| **LiteLLM** | Yes (with `/anthropic` route) | Must be explicitly configured to proxy Anthropic format |
| **vLLM** | No | OpenAI format only |
| **OpenAI** | No | Own format only |

### DeepSeek dual-endpoint caveat

DeepSeek exposes two separate endpoints with **different base URLs and
model names** but the **same API key**:

| Format | Base URL | Model name |
|---|---|---|
| OpenAI (Codex) | `https://api.deepseek.com` | `deepseek-v4-flash` |
| Anthropic (Claude Code) | `https://api.deepseek.com/anthropic` | `deepseek-chat` |

Using the wrong combination (e.g. Anthropic base URL with
`deepseek-v4-flash`) fails silently or returns malformed responses.

### Verified combinations

| Claude Code version | LLM provider | MCP server version | Result |
|---|---|---|---|
| (sandbox default) | DeepSeek (`deepseek-chat`) | 2.0.2 / 3.1.1 | Pass |

## Test script

Quick smoke test to verify your endpoint supports the Responses API with
namespace tools (for Codex). Run from a machine that can reach the
endpoint:

```bash
# Set these to match your provider
OPENAI_BASE_URL="https://api.deepseek.com"
OPENAI_API_KEY="sk-..."
OPENAI_MODEL="deepseek-v4-flash"

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
ANTHROPIC_BASE_URL="https://api.deepseek.com/anthropic"
ANTHROPIC_API_KEY="sk-..."
ANTHROPIC_MODEL="deepseek-chat"

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
