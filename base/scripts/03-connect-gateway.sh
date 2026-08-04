#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"

GATEWAY_NAME="openshift"
MTLS_DIR="${HOME}/.config/openshell/gateways/${GATEWAY_NAME}/mtls"

echo "Extracting mTLS client certificates..."
mkdir -p "$MTLS_DIR"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"

echo "Starting port-forward in the background (PID will be printed)..."
oc -n "$OPENSHELL_NAMESPACE" port-forward svc/openshell 8080:8080 &
PF_PID=$!
echo "port-forward PID: $PF_PID (kill it when done: kill $PF_PID)"
sleep 2

openshell gateway remove "$GATEWAY_NAME" 2>/dev/null || true
openshell gateway add https://127.0.0.1:8080 --local --name "$GATEWAY_NAME"
openshell status
