#!/usr/bin/env bash
set -euo pipefail
: "${SAW_DIR:?set SAW_DIR to the cloned secure-agent-workspace path}"
: "${OCP_API:?set OCP_API}"
: "${OCP_USER:?set OCP_USER}"
: "${OCP_PASSWORD:?set OCP_PASSWORD}"
: "${GEMINI_API_KEY:?set GEMINI_API_KEY}"
: "${SAW_SANDBOX_OWNER:=alice}"

cd "$SAW_DIR"

echo "==> Logging into cluster..."
oc login --server="$OCP_API" --username="$OCP_USER" --password="$OCP_PASSWORD" \
  --insecure-skip-tls-verify=true

echo ""
echo "==> Running SAW prerequisites check..."
make check-prereqs 2>&1 || echo "WARN: check-prereqs had warnings (continuing)"

echo ""
echo "==> Copying pre-built images to internal registry..."
make copy-images 2>&1 || echo "WARN: copy-images had issues (continuing)"

echo ""
echo "==> Deploying Keycloak with OIDC realm..."
make keycloak-issuer 2>&1

echo ""
echo "==> Generating SSH keys..."
if [[ ! -f "${SAW_SSH_KEY_PATH:-}" ]]; then
  make generate-ssh-keys 2>&1
  echo "SSH keys generated. Set SAW_SSH_KEY_PATH to the private key path in .env."
else
  echo "SSH key already exists at ${SAW_SSH_KEY_PATH}, skipping generation."
fi

echo ""
echo "==> Creating SAW sandbox VM..."
# Governance must be disabled until the governance-interceptor chart is deployed.
# Without this, the gateway crash-loops trying to connect to a nonexistent
# interceptor with a fail_closed policy.
OWNER="$SAW_SANDBOX_OWNER" \
  GOVERNANCE_ENABLED=false \
  PROVIDER=gemini \
  MODEL="${GEMINI_MODEL:-gemini-3.6-flash}" \
  API_KEY="$GEMINI_API_KEY" \
  make openshell-saw-create 2>&1

echo ""
echo "SAW deployment initiated. The setup job will:"
echo "  1. Provision the VM disk from the golden image"
echo "  2. Boot the VM and wait for SSH"
echo "  3. Install gateway and supervisor binaries"
echo "  4. Run nemoclaw onboard (may fail — see step 03)"
echo ""
echo "Monitor progress with:"
echo "  oc logs -n openshell-agents -f job/\$(oc get jobs -n openshell-agents -o name | head -1 | cut -d/ -f2)"
