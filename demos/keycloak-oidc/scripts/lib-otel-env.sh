# Sourced by demos/keycloak-oidc/README.md at every `sandbox exec ...
# claude|codex` call site, to avoid repeating the same literal --env flags
# at each of the ~15 sites. Populates the global array OTEL_ENV_ARGS —
# expand as "${OTEL_ENV_ARGS[@]}" in the same shell that sourced this file
# (same-shell usage only, per docs/env-export-gotchas.md's general rule —
# this is never invoked as a separate process).
#
# Claude Code needs all 7 vars via --env at *exec* time (it has no
# config-file equivalent for tracing config).
# CLAUDE_CODE_PROPAGATE_TRACEPARENT=1 is mandatory here specifically
# because this demo always uses a BYO LLM, not the real Anthropic API —
# traceparent propagation into MCP calls is off by default in that case.
#
# Codex bakes 5 of these into config.toml at *provisioning* time instead
# (see scripts/14-provision-codex-sandbox.sh's [otel] block) and only
# needs OTEL_RESOURCE_ATTRIBUTES via --env, since codex-cli has no TOML
# field for arbitrary resource attributes yet (github.com/openai/codex#30987).

otel_claude_env_args() {  # usage: otel_claude_env_args <workspace> <sandbox>
  OTEL_ENV_ARGS=(
    --env "CLAUDE_CODE_ENABLE_TELEMETRY=1"
    --env "CLAUDE_CODE_ENHANCED_TELEMETRY_BETA=1"
    --env "OTEL_TRACES_EXPORTER=otlp"
    --env "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT=http://audit-collector:4318/v1/traces"
    --env "OTEL_EXPORTER_OTLP_PROTOCOL=http/json"
    --env "OTEL_RESOURCE_ATTRIBUTES=workspace=${1},sandbox=${2}"
    --env "CLAUDE_CODE_PROPAGATE_TRACEPARENT=1"
  )
}

otel_codex_env_args() {  # usage: otel_codex_env_args <workspace> <sandbox>
  OTEL_ENV_ARGS=(
    --env "OTEL_RESOURCE_ATTRIBUTES=workspace=${1},sandbox=${2}"
  )
}
