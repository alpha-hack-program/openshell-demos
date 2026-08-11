#!/usr/bin/env bash
set -euo pipefail
#
# Test an LLM endpoint for API compatibility with Codex and Claude Code.
# Outputs a markdown table summarising which APIs work.
#
# Usage:
#   ./09-test-inference-api.sh \
#     --url  https://your-endpoint/v1 \
#     --model your-model-name \
#     --api-key your-api-key
#
# All three flags are required.

usage() {
  echo "Usage: $0 --url <base-url> --model <model> --api-key <key>" >&2
  exit 1
}

BASE_URL="" MODEL="" API_KEY=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --url)     BASE_URL="$2"; shift 2 ;;
    --model)   MODEL="$2";    shift 2 ;;
    --api-key) API_KEY="$2";  shift 2 ;;
    *)         usage ;;
  esac
done

[[ -z "$BASE_URL" || -z "$MODEL" || -z "$API_KEY" ]] && usage

TIMEOUT=30
PROMPT="What is 2+2? Reply in one word."
TOOL_PROMPT="Calculate tax for income 90000 in Lysmark using the calc_tax tool."

FUNC_TOOL='[{"type":"function","name":"calc_tax","description":"Calculate progressive tax with surcharge","parameters":{"type":"object","properties":{"income":{"type":"number","description":"Income amount"},"municipality":{"type":"string","description":"Municipality name"}},"required":["income","municipality"]}}]'

NS_TOOL='[{"type":"namespace","name":"compatibility","description":"Compatibility Engine MCP Server","tools":[{"type":"function","name":"calc_tax","description":"Calculate progressive tax with surcharge","parameters":{"type":"object","properties":{"income":{"type":"number"},"municipality":{"type":"string"}},"required":["income","municipality"]}}]}]'

pass_fail() {
  local http_code="$1" body="$2"
  if [[ "$http_code" =~ ^2 ]]; then
    if echo "$body" | jq -e '.error' >/dev/null 2>&1; then
      echo "FAIL"
    else
      echo "PASS"
    fi
  else
    echo "FAIL"
  fi
}

has_tool_call() {
  local body="$1" api="$2"
  case "$api" in
    chat)
      echo "$body" | jq -e '.choices[0].message.tool_calls[0]' >/dev/null 2>&1 && echo "yes" || echo "no"
      ;;
    responses)
      echo "$body" | jq -e '.output[] | select(.type == "function_call")' >/dev/null 2>&1 && echo "yes" || echo "no"
      ;;
  esac
}

echo "Testing endpoint: $BASE_URL"
echo "Model: $MODEL"
echo ""

declare -A RESULTS DETAILS

# --- 1. Chat Completions ---
RESP=$(curl -sS -w "\n%{http_code}" --max-time "$TIMEOUT" \
  -X POST "${BASE_URL}/chat/completions" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${API_KEY}" \
  -d "{\"model\":\"${MODEL}\",\"messages\":[{\"role\":\"user\",\"content\":\"${PROMPT}\"}],\"max_tokens\":30}" 2>&1) || true
HTTP=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
RESULTS[chat_completions]=$(pass_fail "$HTTP" "$BODY")
DETAILS[chat_completions]="HTTP $HTTP"

# --- 2. Responses API (no tools) ---
RESP=$(curl -sS -w "\n%{http_code}" --max-time "$TIMEOUT" \
  -X POST "${BASE_URL}/responses" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${API_KEY}" \
  -d "{\"model\":\"${MODEL}\",\"input\":\"${PROMPT}\",\"max_output_tokens\":30}" 2>&1) || true
HTTP=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
RESULTS[responses_basic]=$(pass_fail "$HTTP" "$BODY")
DETAILS[responses_basic]="HTTP $HTTP"

# --- 3. Responses API + function tools ---
RESP=$(curl -sS -w "\n%{http_code}" --max-time "$TIMEOUT" \
  -X POST "${BASE_URL}/responses" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${API_KEY}" \
  -d "{\"model\":\"${MODEL}\",\"input\":\"${TOOL_PROMPT}\",\"max_output_tokens\":300,\"tools\":${FUNC_TOOL}}" 2>&1) || true
HTTP=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
RESULTS[responses_func]=$(pass_fail "$HTTP" "$BODY")
TOOL_CALL=$(has_tool_call "$BODY" "responses")
DETAILS[responses_func]="HTTP $HTTP, tool_call=$TOOL_CALL"

# --- 4. Responses API + namespace tools ---
RESP=$(curl -sS -w "\n%{http_code}" --max-time "$TIMEOUT" \
  -X POST "${BASE_URL}/responses" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer ${API_KEY}" \
  -d "{\"model\":\"${MODEL}\",\"input\":\"${TOOL_PROMPT}\",\"max_output_tokens\":300,\"tools\":${NS_TOOL}}" 2>&1) || true
HTTP=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
RESULTS[responses_ns]=$(pass_fail "$HTTP" "$BODY")
DETAILS[responses_ns]="HTTP $HTTP"

# --- 5. Messages API (Anthropic) ---
RESP=$(curl -sS -w "\n%{http_code}" --max-time "$TIMEOUT" \
  -X POST "${BASE_URL}/messages" \
  -H "Content-Type: application/json" \
  -H "x-api-key: ${API_KEY}" \
  -H "anthropic-version: 2023-06-01" \
  -d "{\"model\":\"${MODEL}\",\"messages\":[{\"role\":\"user\",\"content\":\"${PROMPT}\"}],\"max_tokens\":30}" 2>&1) || true
HTTP=$(echo "$RESP" | tail -1)
BODY=$(echo "$RESP" | sed '$d')
RESULTS[messages]=$(pass_fail "$HTTP" "$BODY")
DETAILS[messages]="HTTP $HTTP"

# --- Output markdown table ---
echo "| API | Status | Details |"
echo "|---|---|---|"
echo "| Chat Completions (\`/v1/chat/completions\`) | ${RESULTS[chat_completions]} | ${DETAILS[chat_completions]} |"
echo "| Responses API — no tools (\`/v1/responses\`) | ${RESULTS[responses_basic]} | ${DETAILS[responses_basic]} |"
echo "| Responses API + \`function\` tools | ${RESULTS[responses_func]} | ${DETAILS[responses_func]} |"
echo "| Responses API + \`namespace\` tools | ${RESULTS[responses_ns]} | ${DETAILS[responses_ns]} |"
echo "| Messages API / Anthropic (\`/v1/messages\`) | ${RESULTS[messages]} | ${DETAILS[messages]} |"
echo ""

# --- Summary ---
echo "### Compatibility"
echo ""
if [[ "${RESULTS[responses_ns]}" == "PASS" ]]; then
  echo "- **Codex 0.146.0 + MCP**: COMPATIBLE (namespace tools supported)"
elif [[ "${RESULTS[responses_func]}" == "PASS" ]]; then
  echo "- **Codex 0.146.0 (model only, no MCP)**: COMPATIBLE"
  echo "- **Codex 0.146.0 + MCP**: NOT COMPATIBLE (namespace tools rejected — upgrade vLLM to >= 0.25.0)"
else
  echo "- **Codex 0.146.0**: NOT COMPATIBLE (Responses API not supported)"
fi

if [[ "${RESULTS[messages]}" == "PASS" ]]; then
  echo "- **Claude Code**: COMPATIBLE (Messages API supported)"
else
  echo "- **Claude Code**: NOT COMPATIBLE (Messages API not supported)"
fi
