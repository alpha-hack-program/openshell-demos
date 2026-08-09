#!/usr/bin/env bash
set -euo pipefail
# Verifies per-user credential isolation by testing all user/MCP-server
# combinations. Only the authorized pairs should succeed (200); the rest
# should be rejected by Envoy's RBAC filter (403).
#
# Expected results:
#   user1 → mcp-server-a  = 200 (user1 holds mcp-server-a-user role)
#   user1 → mcp-server-b  = 403 (user1 lacks mcp-server-b-user role)
#   user2 → mcp-server-a  = 403 (user2 lacks mcp-server-a-user role)
#   user2 → mcp-server-b  = 200 (user2 holds mcp-server-b-user role)
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

USERS=("user1" "user2")
SERVERS=("mcp-server-a" "mcp-server-b")

PASS=0
FAIL=0
ERRORS=""

for USER_ID in "${USERS[@]}"; do
  SANDBOX="demo-${USER_ID}"

  # Check sandbox exists and is ready
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

    HTTP_CODE=$(openshell sandbox exec -n "$SANDBOX" \
      --env "MCP_URL=${MCP_URL}" \
      -- bash -c 'curl -s -o /dev/null -w "%{http_code}" \
        -X POST \
        -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
        -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
        "$MCP_URL"' 2>/dev/null)

    # Determine expected result based on authorized pairs
    if { [[ "$USER_ID" == "user1" ]] && [[ "$SERVER_NAME" == "mcp-server-a" ]]; } ||
       { [[ "$USER_ID" == "user2" ]] && [[ "$SERVER_NAME" == "mcp-server-b" ]]; }; then
      EXPECTED=200
    else
      EXPECTED=403
    fi

    if [[ "$HTTP_CODE" == "$EXPECTED" ]]; then
      echo "PASS  ${USER_ID} → ${SERVER_NAME}  HTTP ${HTTP_CODE} (expected ${EXPECTED})"
      ((PASS++))
    else
      echo "FAIL  ${USER_ID} → ${SERVER_NAME}  HTTP ${HTTP_CODE} (expected ${EXPECTED})"
      ((FAIL++))
      ERRORS="${ERRORS}\n  ${USER_ID} → ${SERVER_NAME}: got ${HTTP_CODE}, expected ${EXPECTED}"
    fi
  done
done

echo
echo "Results: ${PASS} passed, ${FAIL} failed"
if [[ $FAIL -gt 0 ]]; then
  echo -e "Failures:${ERRORS}"
  exit 1
fi
