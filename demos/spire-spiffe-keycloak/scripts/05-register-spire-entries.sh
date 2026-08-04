#!/usr/bin/env bash
set -euo pipefail
# Path B (stretch). [VERIFY] every selector against a running sandbox pod's
# actual namespace/service account/labels before trusting this registration.
: "${SPIRE_TRUST_DOMAIN:?set in .env}"
: "${OPENSHELL_NAMESPACE:?set in root .env}"

SPIRE_POD=$(oc -n spire get pod -l app.kubernetes.io/name=spire-server -o jsonpath='{.items[0].metadata.name}')

oc -n spire exec "$SPIRE_POD" -- \
  spire-server entry create \
  -parentID "spiffe://${SPIRE_TRUST_DOMAIN}/spire/agent/k8s_psat/openshell-cluster/spire-agent" \
  -spiffeID  "spiffe://${SPIRE_TRUST_DOMAIN}/gateway" \
  -selector  "k8s:ns:${OPENSHELL_NAMESPACE}" \
  -selector  "k8s:sa:default"

echo "Registered a SPIFFE ID for the gateway workload. Register per-sandbox"
echo "entries the same way once you've confirmed the sandbox pods' actual"
echo "namespace/service account/label selectors — do not assume they match"
echo "the gateway's."
