#!/usr/bin/env bash
set -euo pipefail
echo "Checking prerequisites..."
for bin in oc helm kubectl openshell; do
  command -v "$bin" >/dev/null || { echo "Missing required tool: $bin" >&2; exit 1; }
done
oc whoami >/dev/null || { echo "Not logged into an OpenShift cluster (oc whoami failed)" >&2; exit 1; }
echo "oc, helm, kubectl, openshell all present; logged in as $(oc whoami)."
echo "Reminder: confirm the 'Agent Sandbox' controller and CRDs are installed before proceeding."
