# Sourced by demos/keycloak-oidc/README.md at every `sandbox exec` call
# site that targets an *audited* sandbox (aud-claude-*, aud-codex-*).
# Populates the global array AUDITOR_ENV_ARGS — expand as
# "${AUDITOR_ENV_ARGS[@]}" in the same shell that sourced this file
# (same-shell usage only, per docs/env-export-gotchas.md's general rule).
#
# Why this exists rather than literal --env flags in the README:
# session-auditor's Stop hook reads a DIFFERENT model/key env var name
# depending on which API style it was provisioned with, and
# scripts/16-provision-audited-sandbox.sh picks that style from
# AUDITOR_PROVIDER_STYLE. Hardcoding AUDITOR_ANTHROPIC_MODEL in the README
# is correct only for the default "anthropic" style; under "openai" the
# hook looks for AUDITOR_OPENAI_MODEL instead, finds nothing, and silently
# skips classification. The failure is invisible in the obvious places —
# the agent turn succeeds, the SessionStart/UserPromptSubmit heartbeats
# still land, and the sandbox still appears on the audit dashboard, but it
# never gets a risk_level, so it just sits green forever as though every
# turn were benign. Deriving the name here keeps the two in step.
#
# The credential itself is NOT passed here: it comes from the
# session-auditor-<style> provider attached at provisioning time, so only
# the non-secret model name and base URL travel through --env.
auditor_env_args() {  # usage: auditor_env_args   (no args; reads .env)
  local style="${AUDITOR_PROVIDER_STYLE:-anthropic}"
  local model_key model_value base_url

  case "$style" in
    anthropic)
      model_key="AUDITOR_ANTHROPIC_MODEL"
      model_value="${ANTHROPIC_MODEL:?set ANTHROPIC_MODEL in .env}"
      base_url="${ANTHROPIC_BASE_URL:?set ANTHROPIC_BASE_URL in .env}"
      ;;
    openai)
      model_key="AUDITOR_OPENAI_MODEL"
      model_value="${OPENAI_MODEL:?set OPENAI_MODEL in .env}"
      base_url="${OPENAI_BASE_URL:?set OPENAI_BASE_URL in .env}"
      ;;
    *)
      echo "lib-auditor-env.sh: unknown AUDITOR_PROVIDER_STYLE '$style' (expected 'anthropic' or 'openai')" >&2
      return 1
      ;;
  esac

  AUDITOR_ENV_ARGS=(
    --env "AUDITOR_LLM_BASE_URL=${base_url}"
    --env "${model_key}=${model_value}"
  )
}
