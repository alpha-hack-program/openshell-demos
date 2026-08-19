#!/usr/bin/env bash
set -euo pipefail
# Verify OpenClaw inference works inside the SAW sandbox.

: "${OPENSHELL_NAMESPACE:=openshell-agents}"
: "${SAW_SANDBOX_NAME:=openclaw-test}"
: "${SAW_SSH_KEY_PATH:?set SAW_SSH_KEY_PATH to the private key}"
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

PASS=0
FAIL=0

run_test() {
  local name="$1"
  local cmd="$2"
  echo ""
  echo "==> Test: $name"
  if OUTPUT=$(ssh_vm "$cmd" 2>&1); then
    echo "$OUTPUT"
    ((PASS++))
    echo "PASS"
  else
    echo "$OUTPUT"
    ((FAIL++))
    echo "FAIL"
  fi
}

echo "=== OpenClaw on SAW — verification ==="

run_test "Gateway running" \
  "systemctl --user is-active openshell-gateway.service"

run_test "Sandbox ready" \
  "openshell sandbox list 2>&1 | grep -q '${SAW_SANDBOX_NAME}' && echo 'Sandbox ${SAW_SANDBOX_NAME} is Ready'"

run_test "Sandbox environment" \
  "openshell sandbox exec -n ${SAW_SANDBOX_NAME} --no-tty -- sh -c '
    echo \"OpenClaw: \$(openclaw --version 2>&1 | tail -1)\"
    echo \"Node: \$(node --version)\"
    echo \"Python: \$(python3 --version 2>&1)\"
    echo \"Git: \$(git --version 2>&1)\"
  '"

run_test "Simple Q&A" \
  "openshell sandbox exec -n ${SAW_SANDBOX_NAME} --no-tty -- \
    openclaw infer model run --model google/${GEMINI_MODEL} \
    --prompt 'What is the capital of France? Reply in one word.' --no-color 2>&1 | \
    grep -v '^\[proxy\]' | grep -v UNDICI | grep -v 'node --trace'"

run_test "Code generation" \
  "openshell sandbox exec -n ${SAW_SANDBOX_NAME} --no-tty -- \
    openclaw infer model run --model google/${GEMINI_MODEL} \
    --prompt 'Write a Python one-liner that prints the sum of 1 to 100.' --no-color 2>&1 | \
    grep -v '^\[proxy\]' | grep -v UNDICI | grep -v 'node --trace'"

run_test "Reasoning" \
  "openshell sandbox exec -n ${SAW_SANDBOX_NAME} --no-tty -- \
    openclaw infer model run --model google/${GEMINI_MODEL} \
    --prompt 'If a car travels 60 miles in 1 hour, how far does it travel in 2.5 hours? Answer with just the number and unit.' \
    --no-color 2>&1 | \
    grep -v '^\[proxy\]' | grep -v UNDICI | grep -v 'node --trace'"

echo ""
echo "=== Results: ${PASS} passed, ${FAIL} failed ==="
if (( FAIL > 0 )); then
  echo "Some tests failed. Check the output above for details."
  exit 1
fi
echo "All tests passed. OpenClaw is running successfully inside the SAW sandbox."
