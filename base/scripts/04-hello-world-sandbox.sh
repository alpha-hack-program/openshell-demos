#!/usr/bin/env bash
set -euo pipefail
# Minimal, demo-agnostic verification: sandbox creation, default-deny network
# policy, and policy hot-reload all work on this cluster. No credentials or
# providers involved.

NAME="hello-world"

echo "==> Creating sandbox '$NAME'..."
openshell sandbox create --name "$NAME" -- bash

echo
echo "==> Testing outbound call (should be BLOCKED by default policy)..."
if openshell sandbox exec -n "$NAME" -- curl -sS https://api.github.com/zen 2>&1; then
  echo "ERROR: call succeeded but should have been blocked!" >&2
  exit 1
fi
echo "Blocked as expected."

echo
echo "==> Updating policy to allow api.github.com for /usr/bin/curl..."
openshell policy update "$NAME" \
  --add-endpoint api.github.com:443:read-only:rest:enforce \
  --binary /usr/bin/curl \
  --wait

echo
echo "==> Retesting outbound call (should now SUCCEED)..."
RESULT=$(openshell sandbox exec -n "$NAME" -- curl -sS https://api.github.com/zen 2>&1)
echo "Response: $RESULT"

echo
echo "==> Cleaning up sandbox..."
openshell sandbox delete "$NAME"
echo "hello-world sandbox deleted. base/ verification complete."
