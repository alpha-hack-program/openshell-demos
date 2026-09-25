# Sourced by hand (or by another script) to point the CURRENT shell's
# `openshell` CLI at a given identity — any banker (alice/bob/charlie) or
# admin — by switching XDG_CONFIG_HOME/XDG_STATE_HOME to that identity's
# own $HOME/.local/state/openshell-demos/oc-<user-id>/{config,state}, the
# same convention every scene/script in this demo uses (see "How to follow
# this guide" in the README). Defines a function rather than running on
# source, matching lib-otel-env.sh's pattern — same-shell usage only, per
# docs/env-export-gotchas.md's general rule: sourcing this in one shell and
# expecting it to affect a different process/terminal will not work.
#
# Usage:
#   source scripts/lib-use-identity.sh
#   use_identity <alice|bob|charlie|admin>
#   openshell whoami   # now reports the requested identity
#
# This only points the CLI at that identity's already-registered gateway
# session — it can't perform the one-time browser OIDC login for you (see
# "Log in as each banker" in the README). It also cross-checks the stored
# gateway's endpoint against this demo's own .env (CLUSTER_APPS_DOMAIN/
# OPENSHELL_NAMESPACE), so a stale login left over from a *different*
# cluster is caught here as a clear error instead of surfacing later as a
# confusing SDK error or an "isn't a member of this workspace" that's
# actually about the wrong cluster entirely — the same check
# scripts/18-run-scene7-audit-demo.sh and
# scripts/19-run-scene7-persona-demo.sh already do inline for the one
# banker they're given; this is that same check, reusable for any
# identity including admin.
#
# Also exports SSL_CERT_FILE from the shared, non-identity-scoped CA
# bundle at $XDG_CACHE_HOME/openshell-demos/keycloak-oidc-ca-bundle.pem
# (falls back to $HOME/.cache — see the README's "OIDC issuer TLS trust"
# section), IF that file already exists and SSL_CERT_FILE isn't already
# set to something else. This function never generates that bundle
# itself — it needs `oc` access against openshift-config-managed, which
# not every identity running this script has (typically admin generates
# it once, per the README). If `openshell whoami` still fails after
# calling use_identity with an OIDC discovery error (not an auth/expiry
# one), that bundle probably doesn't exist yet — go generate it per that
# README section, then call use_identity again.

use_identity() {
  local user_id="${1:?usage: use_identity <alice|bob|charlie|admin>}"

  local script_dir demo_dir
  script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
  demo_dir="$script_dir/.."

  local demo_env="$demo_dir/.env"
  if [[ -f "$demo_env" ]]; then
    set -a; source "$demo_env"; set +a
  fi

  export XDG_CONFIG_HOME="$HOME/.local/state/openshell-demos/oc-${user_id}/config"
  export XDG_STATE_HOME="$HOME/.local/state/openshell-demos/oc-${user_id}/state"

  # Shared across every identity (not per-user like the two exports
  # above) -- see the README's "OIDC issuer TLS trust" section. Only set
  # it if the bundle actually exists and the caller hasn't already
  # pointed SSL_CERT_FILE somewhere else on purpose.
  local ca_bundle="${XDG_CACHE_HOME:-$HOME/.cache}/openshell-demos/keycloak-oidc-ca-bundle.pem"
  if [[ -z "${SSL_CERT_FILE:-}" && -f "$ca_bundle" ]]; then
    export SSL_CERT_FILE="$ca_bundle"
  fi

  local gateway_name="${GATEWAY_NAME:-openshift}"
  local gateway_metadata="$XDG_CONFIG_HOME/openshell/gateways/$gateway_name/metadata.json"
  if [[ ! -f "$gateway_metadata" ]]; then
    cat >&2 <<EOF
${user_id} has never logged into gateway '${gateway_name}' from this
identity (nothing at $gateway_metadata). This function can't run the
browser login for you -- do that first (see "Log in as each banker" in
the README), then call use_identity again.
EOF
    return 1
  fi

  if [[ -n "${OPENSHELL_NAMESPACE:-}" && -n "${CLUSTER_APPS_DOMAIN:-}" ]]; then
    local expected_endpoint="https://openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}:443"
    local registered_endpoint
    registered_endpoint="$(jq -r '.gateway_endpoint' "$gateway_metadata" 2>/dev/null || true)"
    if [[ "$registered_endpoint" != "$expected_endpoint" ]]; then
      cat >&2 <<EOF
${user_id}'s stored gateway registration points at a DIFFERENT cluster
than this demo's own .env expects:
  registered: ${registered_endpoint:-<unreadable>}
  expected:   ${expected_endpoint}
This is a stale login from a previous cluster/run, at
$gateway_metadata -- re-run "Log in as each banker" for ${user_id}
against the current cluster before continuing.
EOF
      return 1
    fi
  fi

  echo "Now acting as ${user_id} (XDG_CONFIG_HOME=$XDG_CONFIG_HOME)." \
    "${SSL_CERT_FILE:+SSL_CERT_FILE=$SSL_CERT_FILE}"
}
