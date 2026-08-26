#!/usr/bin/env bash
# Sync current main into a named branch (e.g. a version branch).
# Usage: branch-sync.sh <branch-name> [--no-push] [--create]
set -euo pipefail

usage() {
  echo "Usage: $0 <branch-name> [--no-push] [--create]" >&2
  echo "  --no-push  don't push the branch to origin after a clean merge (default: push)" >&2
  echo "  --create   create <branch-name> from main if it doesn't exist yet" >&2
  exit 1
}

[ $# -ge 1 ] || usage

BRANCH="$1"; shift
PUSH=1
CREATE=0
for arg in "$@"; do
  case "$arg" in
    --no-push) PUSH=0 ;;
    --create) CREATE=1 ;;
    *) usage ;;
  esac
done

[ "$BRANCH" != "main" ] || { echo "ERROR: target branch must not be 'main'" >&2; exit 1; }

git rev-parse --is-inside-work-tree >/dev/null 2>&1 || { echo "ERROR: not a git repository" >&2; exit 1; }

if [ -n "$(git status --porcelain)" ]; then
  echo "ERROR: working tree has uncommitted changes; commit or stash first" >&2
  exit 1
fi

ORIGINAL_BRANCH="$(git symbolic-ref --short -q HEAD || echo "HEAD")"

git fetch -q origin main "$BRANCH" 2>/dev/null || git fetch -q origin main

git checkout -q main
git merge -q --ff-only origin/main

if git show-ref --verify --quiet "refs/heads/$BRANCH"; then
  git checkout -q "$BRANCH"
  git merge -q --ff-only "origin/$BRANCH" 2>/dev/null || true
elif git show-ref --verify --quiet "refs/remotes/origin/$BRANCH"; then
  git checkout -q -b "$BRANCH" "origin/$BRANCH"
elif [ "$CREATE" -eq 1 ]; then
  git checkout -q -b "$BRANCH" main
  echo "Created '$BRANCH' from main."
else
  git checkout -q "$ORIGINAL_BRANCH" 2>/dev/null || true
  echo "ERROR: branch '$BRANCH' not found locally or on origin. Re-run with --create to create it from main." >&2
  exit 1
fi

if git merge --no-edit -q main; then
  MERGE_RESULT="merged cleanly"
else
  echo "ERROR: merge conflicts merging main into '$BRANCH'. Resolve conflicts on '$BRANCH', then commit." >&2
  exit 2
fi

if [ "$PUSH" -eq 1 ]; then
  git push -q origin "refs/heads/$BRANCH:refs/heads/$BRANCH"
  PUSH_RESULT="pushed to origin/$BRANCH"
else
  PUSH_RESULT="not pushed (--no-push)"
fi

echo "OK: '$BRANCH' synced with main ($MERGE_RESULT); $PUSH_RESULT"
