#!/usr/bin/env bash
set -euo pipefail
# Admin-run convenience wrapper for Scene 7 ("Watching the audit trail
# live") provisioning: runs 16-provision-audited-sandbox.sh once for
# claude and once for codex, for the same banker, wired to the same four
# servers step 5 already gives every banker. Provider and policy
# management stay Platform-Admin operations regardless of workspace (see
# 15-provision-claude-sandbox.sh's header) — run this from admin's own
# terminal/identity, never the banker's. Pair with
# 18-run-scene7-audit-demo.sh, which the banker runs themselves afterward.
#
# Usage: ./17-provision-scene7-both-agents.sh <user-id>
#   e.g. ./17-provision-scene7-both-agents.sh bob
#
# 16 in turn hands off to 15-provision-claude-sandbox.sh /
# 14-provision-codex-sandbox.sh (CLAUDE_IMAGE/CODEX_IMAGE pointed at the
# *-audit image, SANDBOX_PREFIX=aud-) — this script doesn't call 14/15
# directly because 16 also attaches the session-auditor-* provider that
# 14/15 know nothing about; skipping 16 would provision the sandboxes
# without the Stop hook that makes Scene 7's dashboard update at all.
# CLAUDE_AUDIT_IMAGE/CODEX_AUDIT_IMAGE (see util/session-auditor/README.md)
# are validated inside 16 itself, per agent branch — not repeated here.
# Idempotent — see 16's own header for what "tolerates already exists"
# covers.
#
# NOTE: the Codex path is still [VERIFY] per the README's Scene 7 note —
# only the Claude Code path has been confirmed live end to end so far.

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DEMO_DIR="$SCRIPT_DIR/.."
cd "$DEMO_DIR"

DEMO_ENV="$DEMO_DIR/.env"
if [[ -f "$DEMO_ENV" ]]; then
  set -a; source "$DEMO_ENV"; set +a
fi

USER_ID="${1:?usage: $0 <user-id>}"

SERVERS="mcp-portfolio,mcp-crm-calendar,mcp-market-news,mcp-kyc-compliance"

"$SCRIPT_DIR/16-provision-audited-sandbox.sh" "$USER_ID" claude "$SERVERS"
"$SCRIPT_DIR/16-provision-audited-sandbox.sh" "$USER_ID" codex "$SERVERS"

echo
echo "aud-claude-${USER_ID} and aud-codex-${USER_ID} provisioned in workspace ${USER_ID}."
echo "Next: as ${USER_ID} (not admin), run ./18-run-scene7-audit-demo.sh ${USER_ID}"
