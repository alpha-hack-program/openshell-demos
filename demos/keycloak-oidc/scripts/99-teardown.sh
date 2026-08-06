#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"

GATEWAY_NAME="${GATEWAY_NAME:-openshift}"
KEYCLOAK_NAMESPACE="${KEYCLOAK_NAMESPACE:-keycloak}"

echo "=== Teardown: demos/keycloak-oidc ==="
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

# --- Delete Keycloak namespace --------------------------------------------------
if oc get ns "$KEYCLOAK_NAMESPACE" &>/dev/null; then
  echo "Deleting Keycloak namespace $KEYCLOAK_NAMESPACE..."
  oc delete ns "$KEYCLOAK_NAMESPACE"
else
  echo "Keycloak namespace $KEYCLOAK_NAMESPACE does not exist (already removed?)."
fi

# --- Delete demo namespace (removes all remaining resources) --------------------
if oc get ns "$OPENSHELL_NAMESPACE" &>/dev/null; then
  echo "Deleting namespace $OPENSHELL_NAMESPACE..."
  oc delete ns "$OPENSHELL_NAMESPACE"
else
  echo "Namespace $OPENSHELL_NAMESPACE does not exist (already removed?)."
fi

echo
echo "Teardown complete. Cluster and local state for demos/keycloak-oidc have been removed."
