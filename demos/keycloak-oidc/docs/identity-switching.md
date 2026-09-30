# Switching identity in a shell (`scripts/as.sh`)

Every `openshell` identity used in this demo (admin, alice, bob, charlie) is
just a gateway registration under a given `XDG_CONFIG_HOME`/
`XDG_STATE_HOME` pair — see
[How to follow this guide](../README.md#how-to-follow-this-guide) for why
this demo uses that convention instead of a shared session. Re-pointing a
shell at one of those identities, plus getting the right OTel `--env` flags
ready for a `sandbox exec ... claude` call, is normally three separate
lines. `scripts/as.sh` collapses that into one.

## Usage

```bash
source scripts/as.sh <alice|bob|charlie|admin>
openshell whoami   # now reports the requested identity
openshell sandbox exec -n claude-<user-id> --workspace <user-id> \
  "${OTEL_ENV_ARGS[@]}" -- claude ...
```

It must be **sourced, not executed** — like every other `lib-*.sh` file in
this demo, it only defines/calls shell functions in the current process;
running it as a subprocess would set nothing in your actual shell (see
`docs/env-export-gotchas.md`'s general rule on this).

## What it actually does

`as.sh` is a two-line convenience wrapper: it sources
`lib-use-identity.sh` and `lib-otel-env.sh`, then calls their functions for
you — `use_identity "$1"` followed by
`otel_claude_env_args "$1" "claude-$1"`. Both are safe to call regardless
of what you do next (e.g. a `parrot`-only block that never touches
`OTEL_ENV_ARGS`) — `otel_claude_env_args` just builds a literal array from
its two arguments, no external state, no failure mode.

`use_identity` (the part that actually switches identity) does four
things:

1. Sources this demo's `.env` (with `set -a`, so every variable in it —
   `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`, etc. — becomes exported into
   your shell too, not just read internally).
2. Sets `XDG_CONFIG_HOME`/`XDG_STATE_HOME` to that identity's own
   `$HOME/.local/state/openshell-demos/oc-<user-id>/{config,state}`.
3. Exports `SSL_CERT_FILE` from the shared ingress-CA bundle at
   `$XDG_CACHE_HOME/openshell-demos/keycloak-oidc-ca-bundle.pem`, but only
   if that bundle already exists and `SSL_CERT_FILE` isn't already set —
   see the main README's
   [OIDC issuer TLS trust](../README.md#oidc-issuer-tls-trust-self-signed-default-ingress-cert)
   section for what generates that bundle and why it's needed at all.
4. Cross-checks the identity's stored gateway registration against this
   demo's own `.env` (`OPENSHELL_NAMESPACE`/`CLUSTER_APPS_DOMAIN`) and
   fails loudly if they don't match — catching a stale login left over
   from a *different* cluster here, instead of it surfacing later as a
   confusing SDK error or an "isn't a member of this workspace" that's
   actually about the wrong cluster entirely.

## What it can't do

- **It can't log an identity in for the first time.** `use_identity` only
  points the CLI at an *already-registered* gateway session — it reads
  `<XDG_CONFIG_HOME>/openshell/gateways/<gateway-name>/metadata.json` and
  fails with a clear error if that file doesn't exist yet. The one-time
  browser OIDC login still has to happen first: for admin, that's
  [step 2b](../README.md#2b-register-the-gateway-with-the-cli); for a
  banker, that's
  [Log in as each banker](../README.md#log-in-as-each-banker-one-time-per-terminal-before-scene-1).
- **It only sets up OTel args for the default Claude Code sandbox name**,
  `claude-<user-id>` — the convention every Scene in this guide uses. If
  you're targeting a different sandbox (Codex's `codex-<user-id>`, or an
  audited `aud-claude-<user-id>` sandbox), `as.sh` itself isn't the right
  tool — call `use_identity`/`otel_codex_env_args`/`otel_claude_env_args`
  directly instead, e.g.:

  ```bash
  source scripts/lib-use-identity.sh
  source scripts/lib-otel-env.sh
  use_identity bob
  otel_codex_env_args bob codex-bob            # Codex, not Claude
  # or:
  otel_claude_env_args bob aud-claude-bob      # an audited sandbox
  ```

  Both functions repopulate the same `OTEL_ENV_ARGS` array — call the
  right one again before your next `sandbox exec` if you switch sandboxes
  mid-session.
