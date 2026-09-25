# Convenience one-liner over lib-use-identity.sh and lib-otel-env.sh:
# sources both and calls use_identity + otel_claude_env_args for you, so
# switching identity AND getting OTel --env args ready for a
# `sandbox exec ... claude` call is one command instead of three. Must
# still be SOURCED, not executed -- it's just those libraries' own
# functions running one level up, and sourcing is transitive: whatever
# this sources into the current shell (the functions, then their
# exports) lands directly in your shell, exactly as if you'd typed every
# line yourself. Nothing here forks a child process, so nothing gets lost.
#
# otel_claude_env_args is called against the DEFAULT regular Claude Code
# sandbox name, "claude-<user-id>" (the convention every scene in this
# guide uses) -- not codex, not an audited aud-claude-<user-id> sandbox,
# since as.sh only knows the identity, not which sandbox you're about to
# target. If you need one of those instead, both functions stay available
# after sourcing this -- just call the right one yourself, e.g.:
#   otel_codex_env_args "$USER_ID" "codex-$USER_ID"
#   otel_claude_env_args "$USER_ID" "aud-claude-$USER_ID"
# (both repopulate the same OTEL_ENV_ARGS array; call again before your
# next `sandbox exec` if you switch sandboxes mid-session)
#
# Usage:
#   source scripts/as.sh <alice|bob|charlie|admin>
#   openshell whoami   # now reports the requested identity
#   openshell sandbox exec -n claude-<user-id> --workspace <user-id> \
#     "${OTEL_ENV_ARGS[@]}" -- claude ...

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "$script_dir/lib-use-identity.sh"
source "$script_dir/lib-otel-env.sh"
use_identity "$1"
otel_claude_env_args "$1" "claude-$1"
unset script_dir
