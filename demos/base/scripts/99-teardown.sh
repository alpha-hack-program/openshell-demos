#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."

ROOT_ENV="$DEMO_DIR/../../.env"
if [[ -f "$ROOT_ENV" ]]; then
  set -a; source "$ROOT_ENV"; set +a
fi

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

: "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in demos/base/.env}"

GATEWAY_NAME="${GATEWAY_NAME:-openshift}"

echo "=== Teardown: demos/base ==="
echo "Namespace: $OPENSHELL_NAMESPACE"
echo "Gateway:   $GATEWAY_NAME"
echo

# --- Kill any local port-forward ------------------------------------------------
PF_PIDS=$(pgrep -f "port-forward.*svc/openshell.*8080" 2>/dev/null || true)
if [ -n "$PF_PIDS" ]; then
  echo "Stopping port-forward process(es): $PF_PIDS"
  kill $PF_PIDS 2>/dev/null || true
fi

# --- Remove openshell CLI gateway registration ----------------------------------
if openshell gateway list 2>/dev/null | grep -q "$GATEWAY_NAME"; then
  echo "Removing openshell gateway '$GATEWAY_NAME'..."
  openshell gateway remove "$GATEWAY_NAME" || true
fi

# --- Remove local mTLS certificates --------------------------------------------
MTLS_DIR="${HOME}/.config/openshell/gateways/${GATEWAY_NAME}/mtls"
if [ -d "$MTLS_DIR" ]; then
  echo "Removing local mTLS certs from $MTLS_DIR..."
  rm -rf "$MTLS_DIR"
fi

# --- Uninstall Helm release -----------------------------------------------------
if helm list -n "$OPENSHELL_NAMESPACE" -q | grep -q '^openshell$'; then
  echo "Uninstalling Helm release 'openshell'..."
  helm uninstall openshell -n "$OPENSHELL_NAMESPACE"
else
  echo "Helm release 'openshell' not found in $OPENSHELL_NAMESPACE (already removed?)."
fi

# --- Delete namespace (removes all remaining resources) -------------------------
if oc get ns "$OPENSHELL_NAMESPACE" &>/dev/null; then
  echo "Deleting namespace $OPENSHELL_NAMESPACE..."
  oc delete ns "$OPENSHELL_NAMESPACE"
else
  echo "Namespace $OPENSHELL_NAMESPACE does not exist (already removed?)."
fi

echo
echo "Teardown complete. Cluster and local state for demos/base have been removed."
