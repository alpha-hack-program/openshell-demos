#!/usr/bin/env bash
set -euo pipefail
# End-to-end walkthrough: same blocked -> policy applied -> allowed shape as
# base/scripts/04-hello-world-sandbox.sh, extended to show the call is scoped
# to one specific customer's own identity, not a shared service identity.
#
# Usage: ./demo.sh <customer-id>   (run 03-onboard-customer.sh for this
# customer-id first)

CUSTOMER_ID="${1:?usage: $0 <customer-id>}"
NAME="demo-${CUSTOMER_ID}"
HERE="$(dirname "$0")"

openshell sandbox create --name "$NAME" -- bash
echo "Sandbox created with no provider attached yet. Inside it, confirm this is BLOCKED:"
echo "  curl -sS https://api.yourdownstream.example.com/v1/me"
read -rp "Press enter once confirmed and you've exited the sandbox... " _

openshell sandbox provider attach "$NAME" "customer-${CUSTOMER_ID}"
openshell policy set "$NAME" --policy "$HERE/../policies/customer-api-readonly.yaml" --wait

echo "Provider and policy applied. Reconnect and confirm the call now succeeds,"
echo "scoped to customer ${CUSTOMER_ID}'s own identity:"
echo "  openshell sandbox connect $NAME"
echo "  curl -sS https://api.yourdownstream.example.com/v1/me"
echo
echo "Isolation check: repeat this whole script with a different customer-id"
echo "while this sandbox is still running, and confirm neither sandbox can see"
echo "the other customer's data."
