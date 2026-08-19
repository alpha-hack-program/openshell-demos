#!/usr/bin/env bash
set -euo pipefail
# Workaround: manually configure inference when nemoclaw onboard fails.
#
# The SAW setup job tries to pip install openshell==0.0.99+rhaiv.0 which does
# not exist on PyPI. This causes the nemoclaw onboard step to fail, leaving
# the sandbox without a configured inference provider. This script replicates
# the onboard steps manually.

: "${OPENSHELL_NAMESPACE:=openshell-agents}"
: "${SAW_SANDBOX_NAME:=openclaw-test}"
: "${SAW_SSH_KEY_PATH:?set SAW_SSH_KEY_PATH to the private key}"
: "${GEMINI_API_KEY:?set GEMINI_API_KEY}"
: "${GEMINI_MODEL:=gemini-3.6-flash}"

VM_NAME="${SAW_SANDBOX_NAME%%-*}"
VM_NAME="${VM_NAME:-mschimun-test}"

ssh_vm() {
  virtctl -n "$OPENSHELL_NAMESPACE" ssh \
    --identity-file="$SAW_SSH_KEY_PATH" \
    cloud-user@vm/"$VM_NAME" \
    --local-ssh-opts="-oStrictHostKeyChecking=no" \
    --local-ssh-opts="-oUserKnownHostsFile=/dev/null" \
    --command="$1"
}

echo "==> Step 1: Get OIDC token from Keycloak..."
KC_HOST=$(oc get route -n "$OPENSHELL_NAMESPACE" -l app=keycloak \
  -o jsonpath='{.items[0].spec.host}' 2>/dev/null || \
  oc get route -n "$OPENSHELL_NAMESPACE" -o jsonpath='{range .items[*]}{.spec.host}{"\n"}{end}' | grep keycloak | head -1)

if [[ -z "$KC_HOST" ]]; then
  echo "ERROR: Could not find Keycloak route in namespace $OPENSHELL_NAMESPACE" >&2
  exit 1
fi

ISSUER="https://${KC_HOST}/realms/openshell"
TOKEN_RESPONSE=$(curl -sk -X POST "${ISSUER}/protocol/openid-connect/token" \
  -d "grant_type=password" \
  -d "client_id=openshell-cli" \
  -d "username=alice" \
  -d "password=alice" \
  -d "scope=openid")

OIDC_TOKEN=$(echo "$TOKEN_RESPONSE" | python3 -c "import json,sys; print(json.load(sys.stdin)['access_token'])" 2>/dev/null)
if [[ -z "$OIDC_TOKEN" ]]; then
  echo "ERROR: Failed to get OIDC token. Response: $TOKEN_RESPONSE" >&2
  exit 1
fi
echo "OIDC token obtained (${#OIDC_TOKEN} chars)"

echo ""
echo "==> Step 2: Write OIDC token and grant workspace access..."
ssh_vm "
OIDC_TOKEN_PATH=\${HOME}/.config/openshell/gateways/openshell/oidc_token.json
mkdir -p \$(dirname \${OIDC_TOKEN_PATH})
chmod 700 \$(dirname \${OIDC_TOKEN_PATH})
printf '{\"access_token\":\"%s\",\"issuer\":\"%s\",\"client_id\":\"openshell-cli\"}' \
  '${OIDC_TOKEN}' '${ISSUER}' > \${OIDC_TOKEN_PATH}
chmod 600 \${OIDC_TOKEN_PATH}
echo 'OIDC token configured'
openshell workspace member add --workspace default --subject openshell-client --role admin 2>&1 || \
  echo '(may already be a member)'
"

echo ""
echo "==> Step 3: Update sandbox network policy..."
ssh_vm "
openshell policy update ${SAW_SANDBOX_NAME} \
  --add-endpoint 'generativelanguage.googleapis.com:443:read-write:rest:enforce' \
  --binary /usr/local/bin/node \
  --binary /usr/bin/node \
  --binary /usr/bin/curl \
  --wait
"

echo ""
echo "==> Step 4: Configure OpenClaw for Gemini..."
ssh_vm "
openshell sandbox exec -n ${SAW_SANDBOX_NAME} --no-tty -- sh -c 'cat > /sandbox/.openclaw/agents/main/agent/models.json << MODEOF
{
  \"providers\": {
    \"google\": {
      \"apiKey\": \"${GEMINI_API_KEY}\",
      \"api\": \"google-generative-ai\",
      \"models\": [
        {
          \"id\": \"${GEMINI_MODEL}\",
          \"name\": \"Gemini 3.6 Flash\",
          \"reasoning\": true,
          \"input\": [\"text\", \"image\"],
          \"contextWindow\": 1048576,
          \"maxTokens\": 65536
        }
      ]
    }
  }
}
MODEOF
echo \"models.json updated\"'

openshell sandbox exec -n ${SAW_SANDBOX_NAME} --no-tty -- node -e \"
const fs = require('fs');
const cfg = JSON.parse(fs.readFileSync('/sandbox/.openclaw/openclaw.json', 'utf8'));
cfg.agents.defaults.model.primary = 'google/${GEMINI_MODEL}';
cfg.models.mode = 'replace';
cfg.models.providers = {
  google: {
    apiKey: '${GEMINI_API_KEY}',
    api: 'google-generative-ai',
    models: [{
      id: '${GEMINI_MODEL}',
      name: 'Gemini 3.6 Flash',
      reasoning: true,
      input: ['text', 'image'],
      contextWindow: 1048576,
      maxTokens: 65536
    }]
  }
};
fs.writeFileSync('/sandbox/.openclaw/openclaw.json', JSON.stringify(cfg, null, 2));
console.log('openclaw.json updated — primary model: google/${GEMINI_MODEL}');
\"
"

echo ""
echo "Inference workaround applied. Run 03-verify-openclaw.sh to test."
