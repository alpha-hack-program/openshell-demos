#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"
: "${OPENSHELL_CHART_VERSION:?set in .env, pin a real version before running}"

SCRIPT_DIR="$(dirname "$0")"

USE_CERT_MANAGER="${CERT_MANAGER:-false}"
USE_ROUTE="${OPENSHELL_ROUTE:-false}"
EXTRA_SETS=()

# --- Choose values file and compute SANs ----------------------------------------
if [[ "$USE_CERT_MANAGER" == "true" ]]; then
  VALUES_FILE="${SCRIPT_DIR}/../helm/values-openshift-certmanager.yaml"
  EXTRA_SETS+=(
    --set "certManager.serverDnsNames[2]=openshell.${OPENSHELL_NAMESPACE}.svc"
    --set "certManager.serverDnsNames[3]=openshell.${OPENSHELL_NAMESPACE}.svc.cluster.local"
  )
  echo "cert-manager path enabled — TLS certificates will be managed by cert-manager."
  echo "  PKI init job will only generate JWT signing keys."
  echo "  Server cert SANs (from values file): openshell, 127.0.0.1"
  echo "  Server cert SANs (computed):         openshell.${OPENSHELL_NAMESPACE}.svc"
  echo "                                       openshell.${OPENSHELL_NAMESPACE}.svc.cluster.local"
else
  VALUES_FILE="${SCRIPT_DIR}/../helm/values-openshift.yaml"
fi

# --- Passthrough Route -----------------------------------------------------------
if [[ "$USE_ROUTE" == "true" ]]; then
  : "${CLUSTER_APPS_DOMAIN:?set in .env when OPENSHELL_ROUTE=true}"
  ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
  if [[ "$USE_CERT_MANAGER" == "true" ]]; then
    EXTRA_SETS+=(
      --set "certManager.serverDnsNames[4]=${ROUTE_HOST}"
    )
    echo "                                       ${ROUTE_HOST}"
  else
    EXTRA_SETS+=(
      --set "pkiInitJob.serverDnsNames[0]=${ROUTE_HOST}"
    )
  fi
  EXTRA_SETS+=(
    --set "server.auth.allowUnauthenticatedUsers=true"
  )
  echo "Route path enabled — server cert will include SAN: ${ROUTE_HOST}"
fi

echo ""
helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" \
  --namespace "$OPENSHELL_NAMESPACE" \
  -f "$VALUES_FILE" \
  "${EXTRA_SETS[@]+"${EXTRA_SETS[@]}"}"

oc -n "$OPENSHELL_NAMESPACE" rollout status statefulset/openshell
