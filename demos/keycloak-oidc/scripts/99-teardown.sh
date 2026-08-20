#!/usr/bin/env bash
set -euo pipefail
# Usage: ./99-teardown.sh <full|keep-keycloak>
#   full          - also deletes the Keycloak namespace/realm
#   keep-keycloak - leaves Keycloak in place for fast re-iteration via
#                   ./02-apply-oidc-overlay.sh onward

usage() {
  echo "Usage: $0 <full|keep-keycloak>" >&2
  exit 1
}

[[ $# -eq 1 ]] || usage
MODE="$1"
case "$MODE" in
  full|keep-keycloak) ;;
  *) usage ;;
esac

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
KEYCLOAK_NAMESPACE="${KEYCLOAK_NAMESPACE:-keycloak}"

echo "=== Teardown: demos/keycloak-oidc ($MODE) ==="
echo "Namespace:          $OPENSHELL_NAMESPACE"
echo "Keycloak namespace: $KEYCLOAK_NAMESPACE"
echo "Gateway:            $GATEWAY_NAME"
echo

# --- Remove openshell CLI gateway registration ----------------------------------
if openshell gateway list 2>/dev/null | grep -q "$GATEWAY_NAME"; then
  echo "Removing openshell gateway '$GATEWAY_NAME'..."
  openshell gateway remove "$GATEWAY_NAME" || true
fi

# --- Kill any local port-forward ------------------------------------------------
PF_PIDS=$(pgrep -f "port-forward.*svc/openshell.*8080" 2>/dev/null || true)
if [ -n "$PF_PIDS" ]; then
  echo "Stopping port-forward process(es): $PF_PIDS"
  kill $PF_PIDS 2>/dev/null || true
fi

# --- Remove local mTLS certificates --------------------------------------------
MTLS_DIR="${HOME}/.config/openshell/gateways/${GATEWAY_NAME}/mtls"
if [ -d "$MTLS_DIR" ]; then
  echo "Removing local mTLS certs from $MTLS_DIR..."
  rm -rf "$MTLS_DIR"
fi

# --- Uninstall MCP servers Helm release -----------------------------------------
if helm list -n "$OPENSHELL_NAMESPACE" -q | grep -q '^mcp-servers$'; then
  echo "Uninstalling Helm release 'mcp-servers'..."
  helm uninstall mcp-servers -n "$OPENSHELL_NAMESPACE"
else
  echo "Helm release 'mcp-servers' not found (already removed?)."
fi

# --- Uninstall OpenShell Helm release -------------------------------------------
if helm list -n "$OPENSHELL_NAMESPACE" -q | grep -q '^openshell$'; then
  echo "Uninstalling Helm release 'openshell'..."
  helm uninstall openshell -n "$OPENSHELL_NAMESPACE"
else
  echo "Helm release 'openshell' not found (already removed?)."
fi

# --- Uninstall SPIRE (deployed to its own namespace) ----------------------------
if helm list -n spire -q 2>/dev/null | grep -q '^spire-agent$'; then
  echo "Uninstalling Helm release 'spire-agent'..."
  helm uninstall spire-agent -n spire
fi
if helm list -n spire -q 2>/dev/null | grep -q '^spire-server$'; then
  echo "Uninstalling Helm release 'spire-server'..."
  helm uninstall spire-server -n spire
fi
if oc get ns spire &>/dev/null; then
  echo "Deleting namespace spire..."
  oc delete ns spire
fi

# --- Delete Keycloak namespace (only in full mode) ------------------------------
if [[ "$MODE" == "full" ]]; then
  if oc get ns "$KEYCLOAK_NAMESPACE" &>/dev/null; then
    echo "Deleting Keycloak namespace $KEYCLOAK_NAMESPACE..."
    oc delete ns "$KEYCLOAK_NAMESPACE"
  else
    echo "Keycloak namespace $KEYCLOAK_NAMESPACE does not exist (already removed?)."
  fi
else
  echo "Keeping Keycloak namespace $KEYCLOAK_NAMESPACE (mode: keep-keycloak)."
fi

# --- Delete demo namespace (removes all remaining resources) --------------------
if oc get ns "$OPENSHELL_NAMESPACE" &>/dev/null; then
  echo "Deleting namespace $OPENSHELL_NAMESPACE..."
  oc delete ns "$OPENSHELL_NAMESPACE"
else
  echo "Namespace $OPENSHELL_NAMESPACE does not exist (already removed?)."
fi

echo
if [[ "$MODE" == "full" ]]; then
  echo "Teardown complete. Cluster and local state for demos/keycloak-oidc have been removed."
else
  echo "Light teardown complete. Keycloak namespace/realm left untouched — re-run"
  echo "from step 02 (./02-apply-oidc-overlay.sh) to rebuild the OpenShell side."
fi
