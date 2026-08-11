#!/usr/bin/env bash
set -euo pipefail
# Verifies per-user credential isolation by testing all user/MCP-server
# combinations. Each authorized pair sends a domain-specific MCP tool call;
# unauthorized pairs should be rejected by Envoy's RBAC filter (403).
#
# Expected results:
#   user1 → mcp-server-a  = 200  call evaluate_unpaid_leave_eligibility
#   user1 → mcp-server-b  = 403  (user1 lacks mcp-server-b-user role)
#   user2 → mcp-server-a  = 403  (user2 lacks mcp-server-a-user role)
#   user2 → mcp-server-b  = 200  call calc_tax
#
# Prerequisites:
#   - Both users onboarded (step 3) with providers attached to their sandboxes
#   - Both MCP servers deployed (step 4)
#   - Network policies added for both users to both servers
#
# Usage: ./08-verify-isolation.sh

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"

# Each authorized pair: USER_ID, SERVER_NAME, TOOL_NAME, TOOL_CALL
# user1 → mcp-server-a: Eligibility Engine — evaluate_unpaid_leave_eligibility
#   params: relationship (father/mother/…), situation (illness/birth/…),
#           is_single_parent (bool), total_children_after (int)
# user2 → mcp-server-b: Compatibility Engine — calc_tax
PAIRS=(
  "user1|mcp-server-a|evaluate_unpaid_leave_eligibility|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"evaluate_unpaid_leave_eligibility\",\"arguments\":{\"relationship\":\"mother\",\"situation\":\"illness\",\"is_single_parent\":false,\"total_children_after\":0}}}"
  "user2|mcp-server-b|calc_tax|{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\"params\":{\"name\":\"calc_tax\",\"arguments\":{\"income\":\"90000\"}}}"
)

USERS=("user1" "user2")
SERVERS=("mcp-server-a" "mcp-server-b")

MCP_INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"test","version":"0.1"}}}'

get_pair_info() {
  local user="$1" server="$2"
  for pair in "${PAIRS[@]}"; do
    IFS='|' read -r p_user p_server p_tool p_call <<< "$pair"
    if [[ "$p_user" == "$user" ]] && [[ "$p_server" == "$server" ]]; then
      TOOL_NAME="$p_tool"
      TOOL_CALL="$p_call"
      return 0
    fi
  done
  return 1
}

mcp_request() {
  local sandbox="$1" mcp_url="$2" body="$3"
  openshell sandbox exec -n "$sandbox" \
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

PASS=0
FAIL=0
ERRORS=""

for USER_ID in "${USERS[@]}"; do
  SANDBOX="demo-${USER_ID}"

  if ! openshell sandbox get "$SANDBOX" &>/dev/null; then
    echo "SKIP  ${USER_ID} — sandbox ${SANDBOX} not found"
    continue
  fi

  for SERVER_NAME in "${SERVERS[@]}"; do
    MCP_URL="http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

    # Ensure the sandbox has a network policy for this server
    openshell policy update "$SANDBOX" \
      --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
      --binary /usr/bin/curl --wait &>/dev/null || true

    # Determine if this is an authorized pair
    if get_pair_info "$USER_ID" "$SERVER_NAME"; then
      EXPECTED=200

      # Authorized: initialize the session, then call the domain-specific tool
      HTTP_CODE=$(mcp_request "$SANDBOX" "$MCP_URL" "$MCP_INIT")
      if [[ "$HTTP_CODE" == "200" ]]; then
        HTTP_CODE=$(mcp_request "$SANDBOX" "$MCP_URL" "$TOOL_CALL")
        LABEL="${USER_ID} → ${SERVER_NAME} (${TOOL_NAME})"
      else
        LABEL="${USER_ID} → ${SERVER_NAME} (initialize failed)"
      fi
    else
      EXPECTED=403
      LABEL="${USER_ID} → ${SERVER_NAME}"

      # Unauthorized: initialize is enough — Envoy rejects before the app sees it
      HTTP_CODE=$(mcp_request "$SANDBOX" "$MCP_URL" "$MCP_INIT")
    fi

    if [[ "$HTTP_CODE" == "$EXPECTED" ]]; then
      echo "PASS  ${LABEL}  HTTP ${HTTP_CODE} (expected ${EXPECTED})"
      ((PASS++))
    else
      echo "FAIL  ${LABEL}  HTTP ${HTTP_CODE} (expected ${EXPECTED})"
      ((FAIL++))
      ERRORS="${ERRORS}\n  ${LABEL}: got ${HTTP_CODE}, expected ${EXPECTED}"
    fi
  done
done

echo
echo "Results: ${PASS} passed, ${FAIL} failed"
if [[ $FAIL -gt 0 ]]; then
  echo -e "Failures:${ERRORS}"
  exit 1
fi
