#!/usr/bin/env bash
set -euo pipefail
# Verifies per-banker credential isolation for Meridian Private Bank's three
# bankers (alice, bob, charlie) across the five MCP servers. Two isolation
# mechanisms are checked:
#
#   1. Role-based (Envoy jwt_authn + rbac sidecar, HTTP-level 403): all three
#      bankers hold `banker` (composite over mcp-portfolio-user/
#      mcp-crm-calendar-user/mcp-market-news-user/mcp-kyc-compliance-user),
#      so all three reach those four servers. Only Alice holds
#      `compatibility-user` (via the compatibility-users group), so only
#      she reaches mcp-compatibility.
#   2. Tenant-based (mcp-portfolio's/mcp-kyc-compliance's own
#      assert_owns_client, JSON-RPC-level error inside an HTTP 200 —
#      [VERIFY], not confirmed against a live cluster): Bob, probing the
#      isolation boundary while a promotion decision looms, tries
#      get_positions/get_risk_profile against Alice's and Charlie's
#      client_ids and should get the same ambiguous "not found for caller"
#      error he'd get for a nonexistent client_id, never their data.
#
# Expected results:
#   alice  -> mcp-compatibility   = 200  calc_tax
#   alice  -> mcp-portfolio       = 200  list_my_clients
#   alice  -> mcp-crm-calendar    = 200  get_upcoming_meetings
#   alice  -> mcp-market-news     = 200  get_relevant_news
#   alice  -> mcp-kyc-compliance  = 200  get_risk_profile
#   bob    -> mcp-compatibility   = 403  (bob lacks compatibility-user role)
#   bob    -> mcp-portfolio       = 200  list_my_clients
#   bob    -> mcp-crm-calendar    = 200  get_upcoming_meetings
#   bob    -> mcp-market-news     = 200  get_relevant_news
#   bob    -> mcp-kyc-compliance  = 200  get_risk_profile
#   charlie -> mcp-compatibility  = 403  (charlie lacks compatibility-user role)
#   charlie -> mcp-portfolio      = 200  list_my_clients
#   charlie -> mcp-crm-calendar   = 200  get_upcoming_meetings
#   charlie -> mcp-market-news    = 200  get_relevant_news
#   charlie -> mcp-kyc-compliance = 200  get_risk_profile
#   bob probing cli-004 (Alice's Elena Duarte) via get_positions/get_risk_profile = denied
#   bob probing cli-005 (Charlie's Fundación Iris) via get_positions/get_risk_profile = denied
#
# Prerequisites:
#   - alice, bob, charlie onboarded (step 3) with providers attached to
#     their sandboxes
#   - All five MCP servers deployed (step 4)
#   - Network policies added for each banker to each server they're
#     authorized for (07-authorize-mcp-user.sh) — this script also adds
#     them itself, best-effort, before each call
#
# Usage: ./08-verify-isolation.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"

# Each authorized pair: BANKER_ID, SERVER_NAME, TOOL_NAME, TOOL_CALL
PAIRS=(
  "alice|mcp-compatibility|calc_tax|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"calc_tax\",\"arguments\":{\"income\":\"90000\"}}}"
  "alice|mcp-portfolio|list_my_clients|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"list_my_clients\",\"arguments\":{}}}"
  "alice|mcp-crm-calendar|get_upcoming_meetings|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_upcoming_meetings\",\"arguments\":{}}}"
  "alice|mcp-market-news|get_relevant_news|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_relevant_news\",\"arguments\":{\"tickers\":[],\"sectors\":[\"technology\"]}}}"
  "bob|mcp-portfolio|list_my_clients|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"list_my_clients\",\"arguments\":{}}}"
  "bob|mcp-crm-calendar|get_upcoming_meetings|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_upcoming_meetings\",\"arguments\":{}}}"
  "bob|mcp-market-news|get_relevant_news|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_relevant_news\",\"arguments\":{\"tickers\":[],\"sectors\":[\"logistics\"]}}}"
  "charlie|mcp-portfolio|list_my_clients|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"list_my_clients\",\"arguments\":{}}}"
  "charlie|mcp-crm-calendar|get_upcoming_meetings|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_upcoming_meetings\",\"arguments\":{}}}"
  "charlie|mcp-market-news|get_relevant_news|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_relevant_news\",\"arguments\":{\"tickers\":[],\"sectors\":[\"health\"]}}}"
  "alice|mcp-kyc-compliance|get_risk_profile|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_risk_profile\",\"arguments\":{\"client_id\":\"cli-004\"}}}"
  "bob|mcp-kyc-compliance|get_risk_profile|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_risk_profile\",\"arguments\":{\"client_id\":\"cli-001\"}}}"
  "charlie|mcp-kyc-compliance|get_risk_profile|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"get_risk_profile\",\"arguments\":{\"client_id\":\"cli-005\"}}}"
)

BANKERS=("alice" "bob" "charlie")
SERVERS=("mcp-compatibility" "mcp-portfolio" "mcp-crm-calendar" "mcp-market-news" "mcp-kyc-compliance")

MCP_INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}'

get_pair_info() {
  local banker="$1" server="$2"
  for pair in "${PAIRS[@]}"; do
    IFS='|' read -r p_banker p_server p_tool p_call <<< "$pair"
    if [[ "$p_banker" == "$banker" ]] && [[ "$p_server" == "$server" ]]; then
      TOOL_NAME="$p_tool"
      TOOL_CALL="$p_call"
      return 0
    fi
  done
  return 1
}

mcp_request() {
  local sandbox="$1" mcp_url="$2" body="$3" workspace="$4"
  openshell sandbox exec -n "$sandbox" --workspace "$workspace" \
    --env "MCP_URL=${mcp_url}" \
    --env "BODY=${body}" \
    -- bash -c 'curl -s -o /tmp/mcp_resp -w "%{http_code}" \
      -X POST \
      -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
      -H "Content-Type: application/json" \
      -H "Accept: application/json, text/event-stream" \
      -d "$BODY" \
      "$MCP_URL"' 2>/dev/null
}

mcp_request_body() {
  local sandbox="$1" workspace="$2"
  openshell sandbox exec -n "$sandbox" --workspace "$workspace" \
    -- cat /tmp/mcp_resp 2>/dev/null
}

PASS=0
FAIL=0
ERRORS=""

for BANKER_ID in "${BANKERS[@]}"; do
  SANDBOX="demo-${BANKER_ID}"
  WORKSPACE="${BANKER_ID}"

  if ! openshell sandbox get "$SANDBOX" --workspace "$WORKSPACE" &>/dev/null; then
    echo "SKIP  ${BANKER_ID} — sandbox ${SANDBOX} not found in workspace ${WORKSPACE}"
    continue
  fi

  for SERVER_NAME in "${SERVERS[@]}"; do
    MCP_URL="http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

    # Ensure the sandbox has a network policy for this server
    openshell policy update "$SANDBOX" --workspace "$WORKSPACE" \
      --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
      --binary /usr/bin/curl --wait &>/dev/null || true

    # Determine if this is an authorized pair
    if get_pair_info "$BANKER_ID" "$SERVER_NAME"; then
      EXPECTED=200

      # Authorized: initialize the session, then call the domain-specific tool
      HTTP_CODE=$(mcp_request "$SANDBOX" "$MCP_URL" "$MCP_INIT" "$WORKSPACE")
      if [[ "$HTTP_CODE" == "200" ]]; then
        HTTP_CODE=$(mcp_request "$SANDBOX" "$MCP_URL" "$TOOL_CALL" "$WORKSPACE")
        LABEL="${BANKER_ID} → ${SERVER_NAME} (${TOOL_NAME})"
      else
        LABEL="${BANKER_ID} → ${SERVER_NAME} (initialize failed)"
      fi
    else
      EXPECTED=403
      LABEL="${BANKER_ID} → ${SERVER_NAME}"

      # Unauthorized: initialize is enough — Envoy rejects before the app sees it
      HTTP_CODE=$(mcp_request "$SANDBOX" "$MCP_URL" "$MCP_INIT" "$WORKSPACE")
    fi

    if [[ "$HTTP_CODE" == "$EXPECTED" ]]; then
      echo "PASS  ${LABEL}  HTTP ${HTTP_CODE} (expected ${EXPECTED})"
      ((++PASS))
    else
      echo "FAIL  ${LABEL}  HTTP ${HTTP_CODE} (expected ${EXPECTED})"
      ((++FAIL))
      ERRORS="${ERRORS}\n  ${LABEL}: got ${HTTP_CODE}, expected ${EXPECTED}"
    fi
  done
done

# ---------------------------------------------------------------------------
# Bob's isolation-boundary probe: with a promotion decision looming and his
# book looking thin next to Alice's and Charlie's, Bob tries the client-scoped
# tool on each tenant-isolated server against a client that isn't his.
# Role-based auth (Envoy) doesn't stop this — Bob legitimately holds both
# mcp-portfolio-user and mcp-kyc-compliance-user via `banker`. The isolation
# has to come from each server's own assert_owns_client check instead, which
# returns the same ambiguous "not found for caller" error whether the
# client_id belongs to someone else or doesn't exist at all — so Bob's
# response should be indistinguishable from a typo, never Alice's or
# Charlie's actual data. [VERIFY]: HTTP code assumed 200 (JSON-RPC-level
# error, not an HTTP-level rejection) — not confirmed against a live cluster.
# ---------------------------------------------------------------------------
if openshell sandbox get "demo-bob" --workspace "bob" &>/dev/null; then
  for PROBE_SERVER_TOOL in "mcp-portfolio|get_positions" "mcp-kyc-compliance|get_risk_profile"; do
    IFS='|' read -r PROBE_SERVER PROBE_TOOL <<< "$PROBE_SERVER_TOOL"
    MCP_URL="http://${PROBE_SERVER}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"
    openshell policy update "demo-bob" --workspace "bob" \
      --add-endpoint "${PROBE_SERVER}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
      --binary /usr/bin/curl --wait &>/dev/null || true
    mcp_request "demo-bob" "$MCP_URL" "$MCP_INIT" "bob" >/dev/null

    for probe in "cli-004|Alice's Elena Duarte" "cli-005|Charlie's Fundación Iris"; do
      IFS='|' read -r CLIENT_ID CLIENT_DESC <<< "$probe"
      PROBE_CALL="{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/call\",\"params\":{\"name\":\"${PROBE_TOOL}\",\"arguments\":{\"client_id\":\"${CLIENT_ID}\"}}}"
      mcp_request "demo-bob" "$MCP_URL" "$PROBE_CALL" "bob" >/dev/null
      BODY=$(mcp_request_body "demo-bob" "bob")
      LABEL="bob probing ${CLIENT_ID} (${CLIENT_DESC}) via ${PROBE_SERVER}.${PROBE_TOOL}"
      if echo "$BODY" | grep -qi "no encontrado\|not found"; then
        echo "PASS  ${LABEL} — denied, no cross-tenant data leaked"
        ((++PASS))
      else
        echo "FAIL  ${LABEL} — expected an ownership-denial error, got: ${BODY}"
        ((++FAIL))
        ERRORS="${ERRORS}\n  ${LABEL}: expected denial, got: ${BODY}"
      fi
    done
  done
fi

echo
echo "Results: ${PASS} passed, ${FAIL} failed"
if [[ $FAIL -gt 0 ]]; then
  echo -e "Failures:${ERRORS}"
  exit 1
fi
