#!/usr/bin/env bash
set -euo pipefail
# End-to-end walkthrough: blocked -> policy applied -> allowed.
# Extended to show the call is scoped to one specific user's own identity,
# not a shared service identity.
#
# Usage: ./demo.sh <user-id> <mcp-server-name>
#   (run 03-onboard-user.sh and 07-authorize-mcp-user.sh for this user first)

USER_ID="${1:?usage: $0 <user-id> <mcp-server-name>}"
SERVER_NAME="${2:?usage: $0 <user-id> <mcp-server-name>}"
: "${OPENSHELL_NAMESPACE:?set in .env}"

NAME="demo-${USER_ID}"
MCP_URL="http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox create --name "$NAME" -- bash
echo "Sandbox created with no provider attached yet. Inside it, confirm this is BLOCKED:"
echo "  curl -sS ${MCP_URL}"
read -rp "Press enter once confirmed and you've exited the sandbox... " _

openshell sandbox provider attach "$NAME" "user-${USER_ID}"
openshell policy update "$NAME" \
  --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
  --binary /usr/bin/curl --wait

echo "Provider and policy applied. Reconnect and confirm the call now succeeds,"
echo "scoped to user ${USER_ID}'s own identity:"
echo "  openshell sandbox connect $NAME"
echo "  curl -sS -H \"Authorization: Bearer \$USER_ACCESS_TOKEN\" ${MCP_URL}"
echo
echo "Isolation check: repeat this whole script with a different user-id"
echo "while this sandbox is still running, and confirm neither sandbox can see"
echo "the other user's data."
