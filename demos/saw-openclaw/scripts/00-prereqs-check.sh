#!/usr/bin/env bash
set -euo pipefail
echo "Checking prerequisites for SAW + OpenClaw demo..."

for bin in oc helm kubectl; do
  command -v "$bin" >/dev/null || { echo "Missing required tool: $bin" >&2; exit 1; }
done

oc whoami >/dev/null 2>&1 || { echo "Not logged into an OpenShift cluster (oc whoami failed)" >&2; exit 1; }

echo "Logged in as $(oc whoami) on $(oc whoami --show-server)"
echo ""

# Check OpenShift version — SAW requires 4.22+ for KubeVirt compatibility
OCP_VERSION=$(oc get clusterversion version -o jsonpath='{.status.desired.version}')
echo "OpenShift version: ${OCP_VERSION}"

OCP_MAJOR=$(echo "$OCP_VERSION" | cut -d. -f1)
OCP_MINOR=$(echo "$OCP_VERSION" | cut -d. -f2)
if (( OCP_MAJOR < 4 || (OCP_MAJOR == 4 && OCP_MINOR < 22) )); then
  echo "WARNING: SAW requires OpenShift 4.22+. Current version ${OCP_VERSION} may not work." >&2
fi

# Check for OpenShift Virtualization operator (KubeVirt)
if oc get csv -A 2>/dev/null | grep -q kubevirt-hyperconverged; then
  echo "OpenShift Virtualization: installed"
else
  echo "WARNING: OpenShift Virtualization not found. SAW requires KubeVirt for VM-based sandboxes." >&2
  echo "  Install from OperatorHub: OpenShift Virtualization (cnv)" >&2
fi

# Check for RHBK (Red Hat Build of Keycloak) operator
if oc get packagemanifests -n openshift-marketplace 2>/dev/null | grep -q rhbk-operator; then
  echo "RHBK operator: available in marketplace"
else
  echo "WARNING: rhbk-operator not found in marketplace. SAW uses Keycloak for OIDC." >&2
fi

# Check for virtctl
if command -v virtctl >/dev/null 2>&1; then
  echo "virtctl: $(virtctl version --client --short 2>/dev/null || echo 'installed')"
else
  echo "WARNING: virtctl not found. Download from the cluster's ConsoleCLIDownload resource:" >&2
  echo "  oc get ConsoleCLIDownload virtctl-clidownloads-kubevirt-hyperconverged -o jsonpath='{.spec.links}'" >&2
fi

echo ""
echo "Prerequisites check complete."
echo "Reminder: SAW requires bare-metal OpenShift — ROSA and other managed platforms"
echo "  do not support KubeVirt's nested virtualization requirements."
