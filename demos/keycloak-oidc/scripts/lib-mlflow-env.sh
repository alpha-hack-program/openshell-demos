# Sourced by scripts/19-run-scene7-persona-demo.sh and scripts/parrot.sh
# (and any other call site that drives `claude`/`codex` inside a sandbox
# provisioned with MLflow tracing — see docs/mlflow-tracing.md and
# util/session-auditor/README.md's "MLflow tracing (Codex only)" section)
# to build the global array MLFLOW_ENV_ARGS. Harmless no-op when
# MLFLOW_TRACKING_HOST is unset in .env (empty array) — mirrors
# scripts/15-provision-claude-sandbox.sh's own "omitted entirely, not just
# left blank" convention for the same variable.
#
# Agent-agnostic: Claude Code's MLflow tracing is the Python `mlflow
# autolog claude` Stop-hook; Codex's is session-auditor's own in-process
# trace push (util/session-auditor/src/mlflow.rs). Both read a subset of
# these same seven vars (Claude: the five MLFLOW_CLAUDE_*/shared ones;
# Codex: the two MLFLOW_CODEX_*/shared ones) and ignore the rest — passing
# all seven to every turn regardless of agent is harmless, same as this
# function already did for its original five.
#
# Why all vars travel via --env regardless of which sandbox variant this
# targets: the audited images (claude-audit/codex-audit) bake their MLflow
# Stop hook into a read-only managed-config file with no "env" block of
# its own — it needs these at exec time or not at all. The plain
# (non-audited) claude-sandbox image instead gets
# MLFLOW_TRACKING_URI/MLFLOW_EXPERIMENT_NAME baked into
# /sandbox/.claude/settings.json's own "env" block by the one-time `mlflow
# autolog claude -d /sandbox ...` registration scripts/15 now runs — but
# MLFLOW_WORKSPACE and MLFLOW_TRACKING_INSECURE_TLS have no such
# file-based home either way (the mlflow CLI has no flag for them), so
# this function passes everything uniformly; --env always wins over
# settings.json's own "env" block, so repeating vars that are already
# baked for the non-audited path (or ignored by the other agent) is
# harmless.
mlflow_env_args() {  # usage: mlflow_env_args   (no args; reads .env)
  MLFLOW_ENV_ARGS=()
  [ -n "${MLFLOW_TRACKING_HOST:-}" ] || return 0
  : "${OPENSHELL_NAMESPACE:?set OPENSHELL_NAMESPACE in .env}"
  MLFLOW_ENV_ARGS=(
    --env "MLFLOW_CLAUDE_TRACING_ENABLED=true"
    --env "MLFLOW_CODEX_TRACING_ENABLED=true"
    --env "MLFLOW_TRACKING_URI=https://${MLFLOW_TRACKING_HOST}:${MLFLOW_TRACKING_PORT:-8443}/mlflow"
    --env "MLFLOW_EXPERIMENT_NAME=claude-code-traces"
    --env "MLFLOW_CODEX_EXPERIMENT_NAME=codex-code-traces"
    --env "MLFLOW_WORKSPACE=${OPENSHELL_NAMESPACE}"
    --env "MLFLOW_TRACKING_INSECURE_TLS=true"
  )
}
