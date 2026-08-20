# Headless browser automation for OAuth flows

When running demo guides that involve browser-based OAuth flows (e.g.
`openshell gateway login`, the `onboard` tool), use **Playwright** with
headless Chromium to automate the Keycloak login form.

## Playwright setup

```bash
mkdir -p /tmp/playwright-scratch && cd /tmp/playwright-scratch
npm init -y && npm install playwright
npx playwright install chromium
```

## Keycloak demo credentials

Demo user credentials (usernames, passwords, role assignments) are defined
in each demo's realm export JSON — e.g.
`demos/keycloak-oidc/keycloak/realm-export.json`. Parse that file for
usernames and passwords rather than hardcoding them.

## Preventing real browser popups

The `openshell` CLI uses `xdg-open` (Linux) to launch the browser for
OAuth flows. To intercept the URL without opening Firefox/Chrome, create
fake stubs in a temp directory and prepend it to `PATH`:

```bash
FAKE_BIN_DIR=$(mktemp -d)
for cmd in xdg-open firefox google-chrome chromium-browser open; do
  printf '#!/bin/bash\necho "$1" > /tmp/oauth-url\n' > "$FAKE_BIN_DIR/$cmd"
  chmod +x "$FAKE_BIN_DIR/$cmd"
done
export PATH="$FAKE_BIN_DIR:$PATH"
export DISPLAY=""   # prevent any GUI fallback
```

Then run the CLI command — it writes the URL to `/tmp/oauth-url` instead
of opening a browser. Read that file and drive it with Playwright.

## Browser-based OAuth flows

- **`openshell gateway add`** already triggers the browser-based login
  flow — there is no need to run a separate `openshell gateway login`
  afterward.
- **`openshell gateway login`** has no `--no-browser` flag. Use the
  `xdg-open` interception above to capture the URL, then drive it with
  Playwright. The CLI starts a callback listener on a random localhost
  port — after Playwright submits the Keycloak form, wait for the redirect
  to `localhost` or `127.0.0.1`.
- **The `onboard` tool** supports `--no-browser`, which prints the
  authorization URL to stdout instead of opening a browser. Start the tool
  in a child process, capture the URL, drive the Keycloak login form with
  Playwright, and let the redirect complete to the tool's localhost
  callback listener (`127.0.0.1:9999`).

## Keycloak login form selectors

The Keycloak login page uses these selectors (stable across Keycloak 26+):
- Username: `#username`
- Password: `#password`
- Submit button: `#kc-login`

After submitting, wait for the redirect to `localhost` (the OAuth callback).

## CLI + Playwright orchestration pattern

The fundamental challenge is timing: a CLI command blocks waiting for a
browser callback, so the browser automation must run concurrently with
the CLI process. Use this pattern:

```bash
# 1. Clear any stale URL file
rm -f /tmp/oauth-url

# 2. Start the CLI command in the background
openshell gateway add "$GATEWAY_URL" &
CLI_PID=$!

# 3. Poll for the captured URL (the xdg-open stub writes it)
OAUTH_URL=""
for i in $(seq 1 30); do
  if [[ -f /tmp/oauth-url ]]; then
    OAUTH_URL=$(cat /tmp/oauth-url)
    rm -f /tmp/oauth-url
    break
  fi
  sleep 1
done

if [[ -z "$OAUTH_URL" ]]; then
  echo "ERROR: CLI did not produce an OAuth URL within 30 seconds"
  kill "$CLI_PID" 2>/dev/null
  exit 1
fi

# 4. Drive the Keycloak login form with Playwright
node /tmp/playwright-scratch/keycloak-login.js "$OAUTH_URL" "$USERNAME" "$PASSWORD"

# 5. Wait for the CLI to finish (it completes after the callback)
wait "$CLI_PID"
CLI_EXIT=$?

if [[ $CLI_EXIT -ne 0 ]]; then
  echo "ERROR: CLI exited with code $CLI_EXIT"
  exit 1
fi
```

### Minimal Playwright login script (`keycloak-login.js`)

```javascript
const { chromium } = require('playwright');

const [url, username, password] = process.argv.slice(2);

(async () => {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  await page.goto(url);
  await page.fill('#username', username);
  await page.fill('#password', password);
  await page.click('#kc-login');
  // Wait for the redirect to the localhost callback
  await page.waitForURL(/localhost|127\.0\.0\.1/, { timeout: 15000 });
  await browser.close();
})();
```

## Running multiple CLI identities concurrently on one machine

**Confirmed** (tested against a live cluster running `demos/keycloak-oidc`,
openshell CLI 0.0.106): `XDG_CONFIG_HOME` + `XDG_STATE_HOME` overrides alone
are sufficient to run several fully-isolated `openshell` CLI identities in
the same shell session on a single Linux machine — no separate VMs,
containers, or full `$HOME` swap needed. This is useful for testing
multi-role demos (e.g. an admin plus several end users) headlessly without
juggling incognito windows or separate machines.

Setup:

```bash
ROOT=/tmp/openshell-identities
mkdir -p "$ROOT"/{admin,user1,user2}/{config,state}

# Run every openshell command for a given identity with both vars set:
XDG_CONFIG_HOME="$ROOT/<identity>/config" XDG_STATE_HOME="$ROOT/<identity>/state" \
  openshell <command>
```

Findings from driving three identities (admin + two OIDC users) through
`gateway add` (Playwright-driven login), `whoami`, provider/sandbox
commands, and diffing the resulting trees:

- **`whoami` correctly reports each identity** after all three were logged
  in back-to-back in the same shell — no state bleed between them, even
  though all three registered a gateway under the *same* name (`openshift`)
  independently in their own XDG root.
- **Effective config path is `$XDG_CONFIG_HOME/openshell/...`**, not
  `$XDG_CONFIG_HOME/.config/openshell/...` — i.e. `XDG_CONFIG_HOME` replaces
  what would otherwise be `$HOME/.config`, it doesn't get `.config` appended
  again. Per-identity state lands under
  `$XDG_CONFIG_HOME/openshell/gateways/<name>/{oidc_token.json,metadata.json,mtls/*}`
  and `$XDG_CONFIG_HOME/openshell/active_gateway`.
- **`XDG_STATE_HOME` was not observed to be used** by CLI 0.0.106 for any
  of gateway login, `settings set`, `provider` commands, or `sandbox
  create`/`exec` — every file written landed under `XDG_CONFIG_HOME`
  instead. Keep setting `XDG_STATE_HOME` anyway (the binary does reference
  it via `strings`, and future versions may use it) but don't rely on it
  being where state actually shows up today.
- **`HOME` does not need to be set at all** once both `XDG_CONFIG_HOME` and
  `XDG_STATE_HOME` are exported — verified with `env -u HOME
  XDG_CONFIG_HOME=... XDG_STATE_HOME=... openshell gateway list` running
  cleanly. (The CLI does contain a "HOME is not set" error string, but it's
  only reached on the fallback path when `XDG_CONFIG_HOME` is absent.)
- **No leakage outside the two XDG roots was found**: the real
  `$HOME/.config/openshell` directory (holding unrelated pre-existing
  gateways) was untouched throughout, and `$HOME/.ssh` saw no new files.
  Nothing else under `/tmp` outside the chosen roots was touched either.
- **Gateway RBAC bonus finding**: CLI sessions authenticated as `user1`/
  `user2` (Keycloak `openshell-user` role) were denied *every* gateway
  operation tried, including read-only ones like `sandbox list` — not with
  a role-specific error, but `"not a member of workspace 'default'"`. Only
  the identity holding `openshell-admin` had workspace membership. In this
  demo's current setup, per-user CLI sessions can authenticate but cannot
  self-service anything at the gateway; the admin session does all
  provider/sandbox/policy management on users' behalf.

**Verdict: XDG_CONFIG_HOME/XDG_STATE_HOME scoping alone is sufficient for
concurrent multi-identity `openshell` CLI testing on one machine.** Full
`$HOME` isolation (separate containers/VMs) is not required for this use
case.

## OpenShell CLI quirks

- **Sandbox home directory** is `/sandbox`, not `/home/sandbox`.
- **`sandbox create --from <image>`** — the image is passed via `--from`,
  not as a positional argument.
- **`--upload` path semantics** — takes `<LOCAL_PATH>:<SANDBOX_PATH>`.
  Uploading a directory nests it as a subdirectory inside the target. To
  inject a single file, specify the full file path on both sides:
  `--upload /tmp/config.toml:/sandbox/.codex/config.toml`.
- **`((count++))` under `set -e`** — post-increment from 0 evaluates to 0
  (falsy), which `set -e` treats as a failure. Use pre-increment
  `((++count))` instead.
