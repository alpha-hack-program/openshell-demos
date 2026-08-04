#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"
: "${OPENSHELL_CHART_VERSION:?set in .env, pin a real version before running}"

helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" \
  --namespace "$OPENSHELL_NAMESPACE" \
  -f "$(dirname "$0")/../helm/values-openshift.yaml"

oc -n "$OPENSHELL_NAMESPACE" rollout status statefulset/openshell
