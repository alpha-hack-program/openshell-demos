#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"

GATEWAY_NAME="openshift"
MTLS_DIR="${HOME}/.config/openshell/gateways/${GATEWAY_NAME}/mtls"

# --- Determine connection mode ---------------------------------------------------
# Default: port-forward (official eval path).
# If OPENSHELL_ROUTE=true and CLUSTER_APPS_DOMAIN is set, create a passthrough
# Route instead. This is NOT the official path — see the README section
# "Exposing the gateway via passthrough Route" for details.
# ---------------------------------------------------------------------------------
USE_ROUTE="${OPENSHELL_ROUTE:-false}"

if [[ "$USE_ROUTE" == "true" ]]; then
  : "${CLUSTER_APPS_DOMAIN:?set in .env when OPENSHELL_ROUTE=true}"
  ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
  GATEWAY_URL="https://${ROUTE_HOST}:443"
else
  GATEWAY_URL="https://127.0.0.1:8080"
fi

echo "Extracting mTLS client certificates..."
mkdir -p "$MTLS_DIR"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"

if [[ "$USE_ROUTE" == "true" ]]; then
  echo "Creating passthrough Route (non-official path)..."
  oc -n "$OPENSHELL_NAMESPACE" get route openshell &>/dev/null \
    || oc -n "$OPENSHELL_NAMESPACE" create route passthrough openshell \
         --service=openshell --port=8080 --hostname="${ROUTE_HOST}"
  echo "Route: ${ROUTE_HOST}"
else
  echo "Starting port-forward in the background (PID will be printed)..."
  oc -n "$OPENSHELL_NAMESPACE" port-forward svc/openshell 8080:8080 &
  PF_PID=$!
  echo "port-forward PID: $PF_PID (kill it when done: kill $PF_PID)"
  sleep 2
fi

openshell gateway remove "$GATEWAY_NAME" 2>/dev/null || true
openshell gateway add "$GATEWAY_URL" --local --name "$GATEWAY_NAME"
openshell status
