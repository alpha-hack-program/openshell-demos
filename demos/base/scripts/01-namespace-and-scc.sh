#!/usr/bin/env bash
set -euo pipefail
: "${OPENSHELL_NAMESPACE:?set in .env}"

oc get ns "$OPENSHELL_NAMESPACE" >/dev/null 2>&1 || oc create ns "$OPENSHELL_NAMESPACE"
oc adm policy add-scc-to-user privileged -z openshell-sandbox -n "$OPENSHELL_NAMESPACE"
echo "Namespace $OPENSHELL_NAMESPACE ready, privileged SCC granted to openshell-sandbox service account."
