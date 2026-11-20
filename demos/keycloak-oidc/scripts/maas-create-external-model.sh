#!/usr/bin/env bash
set -euo pipefail
# Creates everything an ExternalModel needs under RHOAI 3.5+'s
# two-resource MaaS pattern -- a credentials Secret, an ExternalProvider,
# an ExternalModel, and the MaaSModelRef that makes it discoverable -- for
# one upstream backend/model/wire-format combination. See
# llm-d-guide/docs/reference/external-models.md for the full schema this
# follows ("Critical Requirements" #1 and #2's worked examples).
#
# A model is reachable (RuntimeHealthy) but not governed
# (GovernanceAttached: False, calls return 403) until its MaaSModelRef is
# also listed in a MaaSSubscription's spec.modelRefs (with
# tokenRateLimits) -- see llm-d-guide's Phase 6. Pass --subscription to
# patch that in too, right after creating the MaaSModelRef; omit it to
# stop after creating the four resources above and patch a subscription
# in yourself later (this script prints the oc patch command either way
# it can be re-run).
#
# A MaaSSubscription's modelRefs alone only sets rate limits -- it does
# NOT grant access. A matching MaaSAuthPolicy (subjects.groups lining up
# with the subscription's owner.groups) is what actually authorizes a
# group to call the model. This script does not create or touch
# MaaSAuthPolicy (which groups should have access is a judgment call, not
# something to infer) -- it prints a reminder when --subscription is used.
#
# The ExternalProvider and its credentials Secret are named deterministically
# from the base URL's host (e.g. api.deepseek.com -> api-deepseek-com-*),
# so running this script again with the SAME base url reuses the same
# Provider+Secret via `oc apply` (create-or-update) -- this is exactly the
# "one ExternalProvider per backend host, every flavour reuses it" pattern
# the reference doc recommends when adding a second apiFormat flavour of
# the same upstream model (see its "Adding multiple apiFormat flavours"
# section). Override either name explicitly with --provider-name /
# --secret-name if you need to diverge (e.g. two distinct credentials
# against the same host).
#
# SECURITY NOTE: the API key is taken as a positional argument, which
# lands in shell history and is visible to other users on the same host
# via `ps` while this script runs. Prefer `"$(cat keyfile)"` or an already
# -exported env var over typing the literal key at a shared terminal.
#
# Usage: ./maas-create-external-model.sh <base-url> <model-name> <api-key> <api-type> <externalmodel-name> [--namespace NS] [--path PATH] [--provider-name NAME] [--secret-name NAME] [--subscription NAME [--subscription-namespace NS] [--token-rate-limit N] [--token-rate-window WINDOW]]
#   <api-type> is one of: messages | openai-responses | openai-chat
#   (the reference doc also lists vertex-messages; omitted here since it
#   wasn't part of what was asked for -- add it the same way if needed)
#
#   e.g. ./maas-create-external-model.sh api.deepseek.com deepseek-flash sk-xxx messages deepseek-flash-anthropic
#   e.g. ./maas-create-external-model.sh api.deepseek.com deepseek-flash sk-xxx openai-chat deepseek-flash \
#          --namespace llm-d-demo --subscription default-subscription
#
# --path overrides the default path guessed from <api-type> (messages ->
# /v1/messages, openai-chat -> /v1/chat/completions, openai-responses ->
# /v1/responses). The reference doc is explicit that path must match what
# the REAL backend exposes for that wire format, and not every backend
# supports all three -- e.g. this demo's own deepseek-flash uses
# /chat/completions (no /v1) for openai-chat, not the generic default
# below. Verify against the provider's own API docs; don't assume the
# default is correct.
#
# --subscription patches an EXISTING MaaSSubscription's spec.modelRefs
# (JSON patch "add to array", same as llm-d-guide/docs/phases/06-maas.md's
# own example) -- the subscription must already exist with a modelRefs
# list (even an empty one) for that patch to apply. --token-rate-limit /
# --token-rate-window set the required per-model tokenRateLimits entry
# (default 1000 tokens per 24h, same default shown there). Window units
# are s/m/h only -- "d" is not supported, use e.g. 24h.
#
# If <api-type> is "messages": the moment any ExternalModel anywhere in
# the cluster uses apiFormat: messages, the gateway turns on x-api-key
# header auth CLUSTER-WIDE, automatically, with no separate flag -- it
# stays on as long as at least one such ExternalModel exists anywhere,
# regardless of namespace/provider, and only turns back off when the LAST
# one is deleted. This script prints a reminder when that applies.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."
cd "$DEMO_DIR"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

NAMESPACE="llm-d-demo"
PATH_OVERRIDE=""
PROVIDER_NAME_OVERRIDE=""
SECRET_NAME_OVERRIDE=""
SUBSCRIPTION=""
SUBSCRIPTION_NAMESPACE="models-as-a-service"
TOKEN_RATE_LIMIT=1000
TOKEN_RATE_WINDOW="24h"
POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --namespace)
      NAMESPACE="${2:?--namespace requires a value}"
      shift 2
      ;;
    --path)
      PATH_OVERRIDE="${2:?--path requires a value}"
      shift 2
      ;;
    --provider-name)
      PROVIDER_NAME_OVERRIDE="${2:?--provider-name requires a value}"
      shift 2
      ;;
    --secret-name)
      SECRET_NAME_OVERRIDE="${2:?--secret-name requires a value}"
      shift 2
      ;;
    --subscription)
      SUBSCRIPTION="${2:?--subscription requires a value}"
      shift 2
      ;;
    --subscription-namespace)
      SUBSCRIPTION_NAMESPACE="${2:?--subscription-namespace requires a value}"
      shift 2
      ;;
    --token-rate-limit)
      TOKEN_RATE_LIMIT="${2:?--token-rate-limit requires a value}"
      shift 2
      ;;
    --token-rate-window)
      TOKEN_RATE_WINDOW="${2:?--token-rate-window requires a value}"
      shift 2
      ;;
    -h|--help)
      echo "Usage: $0 <base-url> <model-name> <api-key> <api-type> <externalmodel-name> [--namespace NS] [--path PATH] [--provider-name NAME] [--secret-name NAME] [--subscription NAME [--subscription-namespace NS] [--token-rate-limit N] [--token-rate-window WINDOW]]"
      exit 0
      ;;
    *)
      POSITIONAL+=("$1")
      shift
      ;;
  esac
done
# "${POSITIONAL[@]+...}" (not bare "${POSITIONAL[@]}") avoids "unbound
# variable" under `set -u` when POSITIONAL is empty on bash 3.2 (macOS's
# default) — same guard used in 15-provision-claude-sandbox.sh.
set -- "${POSITIONAL[@]+"${POSITIONAL[@]}"}"

USAGE="usage: $0 <base-url> <model-name> <api-key> <api-type> <externalmodel-name> [options]"
BASE_URL="${1:?$USAGE}"
MODEL_NAME="${2:?$USAGE}"
API_KEY="${3:?$USAGE}"
API_TYPE="${4:?$USAGE}"
EXTERNALMODEL_NAME="${5:?$USAGE}"

case "$API_TYPE" in
  messages|openai-responses|openai-chat) ;;
  *) echo "invalid api-type '$API_TYPE' (expected messages, openai-responses, or openai-chat)" >&2; exit 1 ;;
esac

if [[ -n "$SUBSCRIPTION" && ! "$TOKEN_RATE_WINDOW" =~ ^[0-9]+[smh]$ ]]; then
  echo "invalid --token-rate-window '$TOKEN_RATE_WINDOW' (units are s/m/h only -- 'd' is not supported, use e.g. 24h)" >&2
  exit 1
fi

# endpoint must be host-only, no scheme, no path (see reference doc) --
# strip both in case a full URL was pasted in by habit.
HOST="$(printf '%s' "$BASE_URL" | sed -E 's|^https?://||; s|/.*||')"

case "$API_TYPE" in
  messages)         DEFAULT_PATH="/v1/messages" ;;
  openai-chat)      DEFAULT_PATH="/v1/chat/completions" ;;
  openai-responses) DEFAULT_PATH="/v1/responses" ;;
esac
MODEL_PATH="${PATH_OVERRIDE:-$DEFAULT_PATH}"

HOST_SLUG="$(printf '%s' "$HOST" | tr '.' '-' | tr '[:upper:]' '[:lower:]')"
PROVIDER_NAME="${PROVIDER_NAME_OVERRIDE:-${HOST_SLUG}-provider}"
SECRET_NAME="${SECRET_NAME_OVERRIDE:-${HOST_SLUG}-credentials}"

# `base64 | tr -d '\n'` strips wrapping regardless of whether the local
# `base64` defaults to wrapped (GNU, 76 cols) or unwrapped (BSD/macOS)
# output -- portable either way.
API_KEY_B64="$(printf '%s' "$API_KEY" | base64 | tr -d '\n')"

echo "Creating/updating in namespace ${NAMESPACE}:"
echo "  Secret:           ${SECRET_NAME}"
echo "  ExternalProvider: ${PROVIDER_NAME} -> ${HOST}"
echo "  ExternalModel:    ${EXTERNALMODEL_NAME} (apiFormat=${API_TYPE}, path=${MODEL_PATH}, targetModel=${MODEL_NAME})"
echo "  MaaSModelRef:     ${EXTERNALMODEL_NAME}"

cat <<EOF | oc apply -f -
apiVersion: v1
kind: Secret
metadata:
  name: ${SECRET_NAME}
  namespace: ${NAMESPACE}
  labels:
    inference.llm-d.ai/ipp-managed: "true"
type: Opaque
data:
  api-key: ${API_KEY_B64}
---
apiVersion: inference.opendatahub.io/v1alpha1
kind: ExternalProvider
metadata:
  name: ${PROVIDER_NAME}
  namespace: ${NAMESPACE}
spec:
  provider: openai
  endpoint: ${HOST}
  auth:
    type: apikey
    secretRef:
      name: ${SECRET_NAME}
---
apiVersion: inference.opendatahub.io/v1alpha1
kind: ExternalModel
metadata:
  name: ${EXTERNALMODEL_NAME}
  namespace: ${NAMESPACE}
spec:
  externalProviderRefs:
  - ref:
      name: ${PROVIDER_NAME}
    targetModel: ${MODEL_NAME}
    apiFormat: ${API_TYPE}
    path: ${MODEL_PATH}
    weight: 100
---
apiVersion: maas.opendatahub.io/v1alpha1
kind: MaaSModelRef
metadata:
  name: ${EXTERNALMODEL_NAME}
  namespace: ${NAMESPACE}
spec:
  modelRef:
    kind: ExternalModel
    name: ${EXTERNALMODEL_NAME}
EOF

if [ -n "$SUBSCRIPTION" ]; then
  echo
  echo "Patching MaaSSubscription ${SUBSCRIPTION} (namespace ${SUBSCRIPTION_NAMESPACE}) to add ${EXTERNALMODEL_NAME}..."
  oc patch maassubscription "$SUBSCRIPTION" -n "$SUBSCRIPTION_NAMESPACE" --type=json -p "$(cat <<JSONPATCH
[{"op":"add","path":"/spec/modelRefs/-","value":{"name":"${EXTERNALMODEL_NAME}","namespace":"${NAMESPACE}","tokenRateLimits":[{"limit":${TOKEN_RATE_LIMIT},"window":"${TOKEN_RATE_WINDOW}"}]}}]
JSONPATCH
)"
  cat <<REMINDER

Reminder: being in a subscription's modelRefs only sets rate limits, it
does NOT grant access by itself. Make sure a MaaSAuthPolicy exists whose
subjects.groups matches ${SUBSCRIPTION}'s owner.groups and lists this
model in its own modelRefs -- otherwise calls will still be denied despite
GovernanceAttached: True.
REMINDER
else
  cat <<REMINDER

Next: this model is RuntimeHealthy but not yet governed
(GovernanceAttached: False is expected right now, not a failure) until
its MaaSModelRef is added to a MaaSSubscription. Re-run this script with
--subscription <name> to do that now, or patch it in yourself:

  oc patch maassubscription <sub> -n ${SUBSCRIPTION_NAMESPACE} --type=json -p '[
    {"op":"add","path":"/spec/modelRefs/-","value":{
       "name":"${EXTERNALMODEL_NAME}","namespace":"${NAMESPACE}",
       "tokenRateLimits":[{"limit":1000,"window":"24h"}]}}
  ]'

...and a matching MaaSAuthPolicy (subjects.groups matching the
subscription's owner.groups) to actually grant access.
REMINDER
fi

if [ "$API_TYPE" = "messages" ]; then
  echo
  echo "Note: apiFormat=messages just turned on x-api-key header auth for the WHOLE gateway, cluster-wide (stays on as long as any such ExternalModel exists anywhere, off only once the last one is deleted)."
fi
