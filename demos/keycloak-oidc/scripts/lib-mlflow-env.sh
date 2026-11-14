# Sourced by scripts/19-run-scene7-persona-demo.sh (and any other call site
# that drives `claude` inside a sandbox provisioned with MLflow tracing —
# see docs/mlflow-tracing.md) to build the global array MLFLOW_ENV_ARGS.
# Harmless no-op when MLFLOW_TRACKING_HOST is unset in .env (empty array) —
# mirrors scripts/15-provision-claude-sandbox.sh's own "omitted entirely,
# not just left blank" convention for the same variable.
#
# Only matters for Claude Code today — mlflow's autolog integration used
# here is Claude-specific, there is no Codex equivalent in this repo.
#
# Why all five vars travel via --env regardless of which sandbox variant
# this targets: the audited image (claude-audit) bakes its MLflow Stop
# hook into /etc/claude-code/managed-settings.json, which is read-only and
# has no "env" block of its own — it needs these at exec time or not at
# all. The plain (non-audited) claude-sandbox image instead gets
# MLFLOW_TRACKING_URI/MLFLOW_EXPERIMENT_NAME baked into
# /sandbox/.claude/settings.json's own "env" block by the one-time `mlflow
# autolog claude -d /sandbox ...` registration scripts/15 now runs — but
# MLFLOW_WORKSPACE and MLFLOW_TRACKING_INSECURE_TLS have no such
# file-based home either way (the mlflow CLI has no flag for them), so
# this function passes all five uniformly; --env always wins over
# settings.json's own "env" block, so repeating the two that are already
# baked for the non-audited path is harmless.
mlflow_env_args() {  # usage: mlflow_env_args   (no args; reads .env)
  MLFLOW_ENV_ARGS=()
  [ -n "${MLFLOW_TRACKING_HOST:-}" ] || return 0
  : "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
  MLFLOW_ENV_ARGS=(
    --env "MLFLOW_CLAUDE_TRACING_ENABLED=true"
    --env "MLFLOW_TRACKING_URI=https://${MLFLOW_TRACKING_HOST}:${MLFLOW_TRACKING_PORT:-8443}/mlflow"
    --env "MLFLOW_EXPERIMENT_NAME=claude-code-traces"
    --env "MLFLOW_WORKSPACE=${OPENSHELL_NAMESPACE}"
    --env "MLFLOW_TRACKING_INSECURE_TLS=true"
  )
}
