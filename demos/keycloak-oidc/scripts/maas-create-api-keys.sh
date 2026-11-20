#!/usr/bin/env bash
set -euo pipefail
# Mints a MaaS API key (sk-oai-*) for every banker against a given MaaS
# subscription, via maas-api's POST /maas-api/v1/api-keys endpoint (RHOAI
# 3.5+'s Models-as-a-Service gateway -- see
# llm-d-guide/docs/phases/06-maas.md Step 8 for the single-user smoke-test
# version of this call, which this script automates per banker).
#
# This MINTS a brand-new key value; it is NOT
# scripts/rotate-llm-keys.sh, which pushes an ALREADY-MINTED key into
# OpenShell's credential store. Pair the two: run this first to get a key,
# drop it into .env, then run rotate-llm-keys.sh to push it into each
# banker's sandbox.
#
# Each banker needs their OWN bearer token to present to maas-api --
# Authorino/Kuadrant reads the caller's identity off that token (via
# TokenReview) and stamps the X-MaaS-Username header the new key is issued
# under, so admin's own token cannot be used to mint someone else's key.
# [VERIFY]: this demo has no previously-confirmed way to obtain a
# per-banker token for THIS OpenShift cluster identity (separate from a
# banker's OpenShell/Keycloak login) -- two strategies are supported, pick
# whichever matches how your bankers actually authenticate to the cluster:
#
#   1. Pre-set <USER>_TOKEN env vars yourself (e.g. ALICE_TOKEN, BOB_TOKEN,
#      CHARLIE_TOKEN -- same naming convention as this demo's
#      ANTHROPIC_API_KEY_ALICE), however you obtained them (oc login as
#      that user then `oc whoami -t`, an existing kubeconfig context,
#      etc). This script prefers these when set.
#   2. Otherwise it mints one itself via `oc create token <sa> -n <ns>`
#      against a per-banker ServiceAccount -- set MAAS_SA_NAMESPACE (and
#      optionally --sa-prefix) and it looks for a ServiceAccount named
#      "<prefix><user-id>". Mirrors this demo's own MLFLOW_TRACKING_TOKEN
#      minting (see docs/mlflow-tracing.md) -- unconfirmed for this
#      specific purpose, [VERIFY] against your cluster.
#
# Usage: ./maas-create-api-keys.sh <subscription-name> [--expires-in-days N] [--sa-namespace NS] [--sa-prefix PREFIX] [--key-name-suffix SUFFIX] [user-id ...]
#   e.g. ./maas-create-api-keys.sh external-premium-subscription
#   e.g. ./maas-create-api-keys.sh external-premium-subscription --expires-in-days 30 bob
#   e.g. ./maas-create-api-keys.sh external-premium-subscription --sa-namespace models-as-a-service --sa-prefix maas-
#
# Requires CLUSTER_APPS_DOMAIN in .env (MAAS_GW defaults to
# maas.$CLUSTER_APPS_DOMAIN -- same convention as CODEX_BASE_URL -- override
# with MAAS_GW directly if your gateway Route uses a different hostname).
# jq is required to parse the response.
#
# Prints "<user>=<key>" lines on stdout, so you can paste them straight
# into .env as ANTHROPIC_API_KEY_<USER>/CODEX_API_KEY_<USER> -- which one
# depends on which model(s) this subscription includes; this script
# doesn't guess that for you. On any failure for one banker, prints a
# clear error for that banker to stderr and continues to the rest, exiting
# non-zero at the end if anything failed -- same convention as
# scripts/rotate-llm-keys.sh.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."
cd "$DEMO_DIR"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

command -v jq >/dev/null || { echo "jq is required" >&2; exit 1; }

EXPIRES_IN_DAYS=90
SA_NAMESPACE="${MAAS_SA_NAMESPACE:-models-as-a-service}"
SA_PREFIX=""
KEY_NAME_SUFFIX="ext-key"
POSITIONAL=()
while [[ $# -gt 0 ]]; do
  case "$1" in
    --expires-in-days)
      EXPIRES_IN_DAYS="${2:?--expires-in-days requires a value}"
      shift 2
      ;;
    --sa-namespace)
      SA_NAMESPACE="${2:?--sa-namespace requires a value}"
      shift 2
      ;;
    --sa-prefix)
      SA_PREFIX="${2:?--sa-prefix requires a value}"
      shift 2
      ;;
    --key-name-suffix)
      KEY_NAME_SUFFIX="${2:?--key-name-suffix requires a value}"
      shift 2
      ;;
    -h|--help)
      echo "Usage: $0 <subscription-name> [--expires-in-days N] [--sa-namespace NS] [--sa-prefix PREFIX] [--key-name-suffix SUFFIX] [user-id ...]"
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
SUBSCRIPTION="${1:?usage: $0 <subscription-name> [options] [user-id ...]}"
shift

if [ $# -gt 0 ]; then
  BANKERS=("$@")
else
  BANKERS=("alice" "bob" "charlie")
fi

: "${CLUSTER_APPS_DOMAIN:?set CLUSTER_APPS_DOMAIN in .env}"
MAAS_GW="${MAAS_GW:-maas.${CLUSTER_APPS_DOMAIN}}"

FAILED=false

# [VERIFY]: see header comment -- the ServiceAccount fallback is inferred
# from this demo's own MLFLOW_TRACKING_TOKEN mechanism, not confirmed
# against a live MaaS deployment for this purpose.
token_for_user() {
  local user="$1" user_uc token_var
  user_uc=$(printf '%s' "$user" | tr '[:lower:]-' '[:upper:]_')
  token_var="${user_uc}_TOKEN"
  if [ -n "${!token_var:-}" ]; then
    printf '%s' "${!token_var}"
    return 0
  fi
  oc create token "${SA_PREFIX}${user}" -n "$SA_NAMESPACE"
}

for USER_ID in "${BANKERS[@]}"; do
  USER_ID_UC=$(printf '%s' "$USER_ID" | tr '[:lower:]-' '[:upper:]_')
  echo "== ${USER_ID} ==" >&2

  USER_TOKEN="$(token_for_user "$USER_ID")" || {
    echo "  FAILED: no ${USER_ID_UC}_TOKEN set and \`oc create token ${SA_PREFIX}${USER_ID} -n ${SA_NAMESPACE}\` failed -- set ${USER_ID_UC}_TOKEN yourself, or check --sa-namespace/--sa-prefix." >&2
    FAILED=true
    continue
  }

  RESPONSE="$(curl -sk -X POST "https://${MAAS_GW}/maas-api/v1/api-keys" \
    -H "Authorization: Bearer ${USER_TOKEN}" \
    -H "Content-Type: application/json" \
    -d "{\"name\":\"${USER_ID}-${KEY_NAME_SUFFIX}\",\"subscription\":\"${SUBSCRIPTION}\",\"expiresInDays\":${EXPIRES_IN_DAYS}}")"

  KEY="$(printf '%s' "$RESPONSE" | jq -re '.key' 2>/dev/null)" || {
    echo "  FAILED: maas-api did not return a key -- response: ${RESPONSE}" >&2
    FAILED=true
    continue
  }

  echo "${USER_ID}=${KEY}"
done

echo >&2
if [ "$FAILED" = true ]; then
  echo "Done, with at least one failure above." >&2
  exit 1
else
  echo "All requested keys minted." >&2
fi
