# Shell variables vs. exported env vars: two live gotchas

This guide leans heavily on `source .env` to pull demo config into the
current shell, then references `$VAR` in later commands. That works fine
as long as everything reading `$VAR` runs *inside that same shell* (a
`curl` call, a `helm --set`, an inline `openshell` invocation). It breaks
silently as soon as a step shells out to a **separate process** — a
standalone binary or a `./script.sh` — because a plain `VAR=value`
assignment is a shell-local variable, not part of the process environment
`fork`/`exec` hands to a child. Only variables that have been `export`ed
(or came from `source`-ing a file that was itself read while `set -a` was
active) are visible to a child process.

Two real instances of this surfaced during live testing of this demo:

## 1. `onboard --keycloak-host` (Step 3a)

`onboard` is a separate Rust binary. It declares `keycloak_host` via clap
as `#[arg(long, env = "KEYCLOAK_HOST")]` with **no default** — clap will
happily read `KEYCLOAK_HOST` from the process environment if the flag
isn't passed, but only if it's actually there.

The README had:

```bash
source .env
...
onboard -u "$USER_ID" --profile providers/user-refresh-profile.yaml
```

`source .env` sets `KEYCLOAK_HOST` as a shell variable in the *parent*
shell. `onboard` runs as a child process and never sees it, so it failed
live with:

```
error: the following required arguments were not provided:
  --keycloak-host <KEYCLOAK_HOST>
```

**Fix chosen:** pass it explicitly on the command line —
`onboard -u "$USER_ID" --keycloak-host "$KEYCLOAK_HOST" --profile ...` —
rather than wrapping `source .env` in `set -a`/`set +a`. Explicit flags
are more readable in a guide meant to be read top-to-bottom, and they
don't silently export *every* var in `.env` (including things like
`ANTHROPIC_API_KEY` from optional recipes) into every child process for
the rest of the shell session.

## 2. `ROUTE_HOST` for `10-bootstrap-onboarding-web-admin.sh` (Step 3b)

`10-bootstrap-onboarding-web-admin.sh` is a standalone script, invoked as
`./scripts/10-bootstrap-onboarding-web-admin.sh` — a separate process. It
guards on `ROUTE_HOST` with:

```bash
: "${ROUTE_HOST:?set to the OpenShell gateway Route host (same value used in step 2b)}"
```

`ROUTE_HOST` isn't stored in `.env` at all — it's *computed* inline back
in [step 2a](../README.md#2a-helm-install) as
`openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}` and used
directly in that same shell (`helm --set`). If you're in a fresh terminal
by the time you reach step 3b, `ROUTE_HOST` doesn't exist yet, and even
after recomputing it, a plain assignment still isn't visible to the child
script:

```
./scripts/10-bootstrap-onboarding-web-admin.sh: line 42: ROUTE_HOST: set to the OpenShell gateway Route host (same value used in step 2b)
```

**Fix:** `export ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"`
before invoking the script.

## General rule for this repo

When adding or reviewing a README step:

- **Same-shell usage** (`curl`, `helm --set`, inline `openshell ...`,
  string interpolation into another command) — a plain `VAR=value` from
  `source .env` is fine.
- **Separate process** (any `./scripts/*.sh`, the `onboard` binary,
  `onboarding-web` itself, anything invoked as its own executable) — the
  variable must either be `export`ed beforehand or passed as an explicit
  CLI flag. Prefer the explicit flag where the tool supports it (see
  Step 3a above for why); fall back to `export` (or `set -a; source
  .env; set +a`, as `scripts/03-onboard-user.sh` and
  `scripts/10-bootstrap-onboarding-web-admin.sh` already do internally)
  when the tool only reads from its process environment.
