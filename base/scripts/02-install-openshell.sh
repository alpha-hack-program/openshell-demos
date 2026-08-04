#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"
: "${OPENSHELL_CHART_VERSION:?set in .env, pin a real version before running}"

# --- Optional: passthrough Route path (non-official) -----------------------------
# Set OPENSHELL_ROUTE=true and CLUSTER_APPS_DOMAIN in .env to include the route
# hostname in the server cert SANs and enable allowUnauthenticatedUsers.
# See the README section "Exposing the gateway via passthrough Route".
# ---------------------------------------------------------------------------------
USE_ROUTE="${OPENSHELL_ROUTE:-false}"
EXTRA_SETS=()

if [[ "$USE_ROUTE" == "true" ]]; then
  : "${CLUSTER_APPS_DOMAIN:?set in .env when OPENSHELL_ROUTE=true}"
  ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
  EXTRA_SETS+=(
    --set "pkiInitJob.serverDnsNames[0]=${ROUTE_HOST}"
    --set "server.auth.allowUnauthenticatedUsers=true"
  )
  echo "Route path enabled — server cert will include SAN: ${ROUTE_HOST}"
fi

helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" \
  --namespace "$OPENSHELL_NAMESPACE" \
  -f "$(dirname "$0")/../helm/values-openshift.yaml" \
  "${EXTRA_SETS[@]+"${EXTRA_SETS[@]}"}"

oc -n "$OPENSHELL_NAMESPACE" rollout status statefulset/openshell
