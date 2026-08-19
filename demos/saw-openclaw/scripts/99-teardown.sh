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

: "${OPENSHELL_NAMESPACE:=openshell-agents}"

echo "=== Teardown: demos/saw-openclaw ==="
echo "Namespace: $OPENSHELL_NAMESPACE"
echo ""

# --- Delete SAW sandbox VM ---
VM_NAME="${SAW_SANDBOX_NAME:-mschimun-test}"
if oc get vm "$VM_NAME" -n "$OPENSHELL_NAMESPACE" &>/dev/null; then
  echo "Deleting VM ${VM_NAME}..."
  oc delete vm "$VM_NAME" -n "$OPENSHELL_NAMESPACE" --wait=false
fi

# --- Delete DataVolumes ---
for dv in $(oc get dv -n "$OPENSHELL_NAMESPACE" -o name 2>/dev/null); do
  echo "Deleting $dv..."
  oc delete "$dv" -n "$OPENSHELL_NAMESPACE" --wait=false || true
done

# --- Uninstall SAW Helm releases ---
for release in $(helm list -n "$OPENSHELL_NAMESPACE" -q 2>/dev/null); do
  echo "Uninstalling Helm release '$release'..."
  helm uninstall "$release" -n "$OPENSHELL_NAMESPACE" || true
done

# --- Delete Keycloak resources ---
oc delete keycloak --all -n "$OPENSHELL_NAMESPACE" 2>/dev/null || true
oc delete keycloakrealmimport --all -n "$OPENSHELL_NAMESPACE" 2>/dev/null || true

# --- Delete namespace ---
if oc get ns "$OPENSHELL_NAMESPACE" &>/dev/null; then
  echo "Deleting namespace $OPENSHELL_NAMESPACE..."
  oc delete ns "$OPENSHELL_NAMESPACE"
else
  echo "Namespace $OPENSHELL_NAMESPACE does not exist (already removed?)."
fi

# --- Clean up local SSH keys ---
if [[ -n "${SAW_SSH_KEY_PATH:-}" && -f "$SAW_SSH_KEY_PATH" ]]; then
  echo "Removing local SSH key at $SAW_SSH_KEY_PATH..."
  rm -f "$SAW_SSH_KEY_PATH" "${SAW_SSH_KEY_PATH}.pub"
fi

echo ""
echo "Teardown complete. Cluster and local state for demos/saw-openclaw have been removed."
