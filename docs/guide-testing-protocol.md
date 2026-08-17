# Guide testing protocol

This document defines how to test a demo README guide end to end. The goal
is to verify the guide itself — that a human following it step by step
would succeed. This is **not** about reaching the end state by any means
necessary; it's about proving the instructions are correct.

## Core rules

1. **Follow the README literally.** Execute each command exactly as
   written, in order. Do not skip steps, combine steps, substitute
   equivalent commands, or reorder. If the guide says `oc new-project`,
   run `oc new-project` — don't use `oc create namespace` because you
   know it also works.

2. **Use the browser when the guide says to.** If a step says "log in via
   the browser" or triggers an OAuth flow, drive it with Playwright as
   described in [`headless-browser-automation.md`](headless-browser-automation.md).
   Do not bypass the browser flow by manually crafting tokens or calling
   APIs directly.

3. **Use the guide's credentials source.** Parse credentials from wherever
   the guide says (e.g. realm export JSON, `.env` file). Do not hardcode
   values you remember from a previous run.

4. **Verify each step's expected outcome.** After running a command, check
   that the output or cluster state matches what the guide says should
   happen. If the guide doesn't state an expected outcome, note that as a
   gap.

## Environment bootstrapping

Before starting, ensure the test environment is ready:

1. **Cluster access.** Confirm `oc whoami` succeeds and you have the
   expected permissions.
2. **Root `.env`.** Source the root `.env` file to get cluster-wide
   variables (`OPENSHELL_CHART_VERSION`, `CLUSTER_APPS_DOMAIN`). If the
   root `.env` doesn't exist, create it from `.env.example` and **ask the
   user for the real values** — don't guess.
3. **Demo `.env`.** Source the demo's own `.env` file (e.g.
   `demos/keycloak-oidc/.env`). Same rule: if it doesn't exist, ask.
4. **Playwright.** Ensure Playwright + Chromium are installed (see
   [`headless-browser-automation.md`](headless-browser-automation.md)).
5. **xdg-open interception.** Set up the fake browser stubs before
   running any CLI command that might trigger an OAuth flow.

## Handling `[VERIFY]` tags

Commands marked `[VERIFY]` in the guide are unconfirmed — they were
inferred, not tested against a live system. During testing:

- **Try the command as written.** If it works, remove the `[VERIFY]` tag
  and note it as confirmed.
- **If it fails**, investigate: is the command wrong, or is the
  environment not ready? Fix the command if wrong, fix the environment if
  that's the issue, and note what changed.
- **Never silently skip** a `[VERIFY]`-tagged step.

## When a step fails

1. **Stop and diagnose.** Don't skip ahead — later steps may depend on
   this one.
2. **Classify the failure:**
   - **Guide bug** — the command is wrong, the expected output is wrong,
     or a prerequisite step is missing. Fix the guide and note the change.
   - **Environment issue** — the cluster is misconfigured, a resource is
     missing, permissions are wrong. Fix the environment (or ask the user)
     and retry the step.
   - **Transient failure** — network timeout, pod not ready yet. Retry
     with a reasonable wait.
3. **Record the failure** with: which step, what the guide said, what
   actually happened, and what you did to resolve it.
4. **After fixing, re-run the step** to confirm the fix before moving on.

## Pass/fail criteria

A guide test **passes** when:
- Every step in the README was executed in order as written
- Every step produced the expected outcome (or the guide was corrected
  to match the actual correct outcome)
- The "Definition of done" section at the end of the guide is fully
  satisfied
- All `[VERIFY]` tags encountered were resolved (confirmed or fixed)

A guide test **fails** when:
- A step cannot be made to work and blocks further progress
- The "Definition of done" criteria are not met after all steps complete

## Reporting

After testing, produce a summary:
- **Result:** pass or fail
- **Guide changes made:** list of corrections (wrong commands, missing
  steps, updated `[VERIFY]` tags)
- **Environment issues encountered:** anything that required cluster-side
  fixes
- **Open issues:** anything unresolved or requiring user input
