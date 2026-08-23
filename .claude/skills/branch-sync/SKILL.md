---
name: branch-sync
description: Merge current main into a named branch (e.g. a version branch like v0.0.106) to keep it up to date with main. Use when the user asks to sync, update, or merge main into a specific branch by name.
---

# Branch Sync

Syncs `main` into a target branch by running one script — do not run the
underlying `git fetch`/`checkout`/`merge` commands yourself, and do not
inspect `git log`/`git status` before or after unless the script reports an
error. This keeps the operation to a single tool call.

## Usage

```bash
.claude/skills/branch-sync/scripts/branch-sync.sh <branch-name> [--push] [--create]
```

- `<branch-name>` — the branch to bring up to date with `main` (e.g.
  `v0.0.106`).
- `--create` — create `<branch-name>` from `main` if it doesn't exist yet
  locally or on `origin`. Without this flag, a missing branch is an error.
- `--push` — push the branch to `origin` after a clean merge. Without this
  flag the branch is left updated locally only.

The script requires a clean working tree and refuses to run if there are
uncommitted changes. It leaves the repo checked out on `<branch-name>` when
it succeeds.

## Reading the result

The script prints exactly one line on success (`OK: ...`) or one line on
failure (`ERROR: ...`) plus a non-zero exit code:

- Exit `0` — synced, message starts with `OK:`.
- Exit `1` — precondition failed (dirty tree, branch missing without
  `--create`, wrong repo). Fix the stated issue and re-run.
- Exit `2` — merge conflict. Do not attempt to resolve automatically unless
  the user asks — report the conflict and let the user decide, since
  conflict resolution can silently drop intended changes.

Report the single output line back to the user rather than re-deriving
status with extra git commands.

## Confirm before pushing

Per this repo's [`AGENTS.md`](../../../AGENTS.md) conventions, don't push to
`origin` without the user's explicit go-ahead for that specific push. Ask
before adding `--push`, unless the user already asked for it in the same
request.
