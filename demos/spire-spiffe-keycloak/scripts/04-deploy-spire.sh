#!/usr/bin/env bash
set -euo pipefail
# Path B (stretch). Every command here is [VERIFY] — see the demo README's
# Open Risks section before running this against a real cluster.
: "${SPIRE_TRUST_DOMAIN:?set in .env}"

helm repo add spiffe https://spiffe.github.io/helm-charts-hardened/
helm repo update

oc get ns spire >/dev/null 2>&1 || oc create ns spire
oc adm policy add-scc-to-user privileged -z spire-agent -n spire   # [VERIFY] service account name

HERE="$(dirname "$0")"
helm upgrade --install spire-server spiffe/spire-server -n spire \
  -f "$HERE/../spire/values-spire-server.yaml"
helm upgrade --install spire-agent spiffe/spire-agent -n spire \
  -f "$HERE/../spire/values-spire-agent.yaml"

echo "SPIRE deployed to trust domain $SPIRE_TRUST_DOMAIN (namespace: spire)."
echo "Next: ./05-register-spire-entries.sh"
