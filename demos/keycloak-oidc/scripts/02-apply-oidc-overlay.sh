#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"
: "${OPENSHELL_CHART_VERSION:?set in .env}"

HERE="$(dirname "$0")"
VALUES="$HERE/../helm/values.yaml"

helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" \
  --namespace "$OPENSHELL_NAMESPACE" \
  -f "$VALUES"

oc -n "$OPENSHELL_NAMESPACE" rollout status statefulset/openshell

openshell settings set --global --key providers_v2_enabled --value true
echo "OIDC overlay applied, Providers v2 enabled. Re-run 'openshell status' — the CLI"
echo "should now be doing a real OIDC login against Keycloak."
