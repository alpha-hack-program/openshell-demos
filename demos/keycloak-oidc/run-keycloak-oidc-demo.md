I need you to run the keycloak-oidc demo from `demos/keycloak-oidc/README.md` end to end, playing three roles: **admin**, **user1**, and **user2**.

**Important constraints:**

- **Don't change any files in the repo** — no edits to READMEs, scripts, helm values, etc.
- **Confirm with me before running any cluster-mutating command** (`oc apply`, `helm install/upgrade`, `oc adm policy`, secret creation, etc.) — show me what you're about to run and wait for my go-ahead.
- Follow the README steps in order (1 through 5), including the MCP server deployment (step 4) and the isolation verification (step 5).

**Browser-based OAuth flows:**

For any step that requires a browser login (e.g. `openshell gateway login`, the `onboard` tool's OAuth callback), use **Playwright** with headless Chromium to automate the Keycloak login form. Playwright is installed at `/tmp/playwright-scratch/` (run scripts with `cd /tmp/playwright-scratch && node -e "..."` or `node /path/to/script.js`). The Chromium binary is cached in `~/.cache/ms-playwright/`.

The Keycloak demo users and their passwords (from `keycloak/realm-export.json`):
- `openshell-admin` / `openshell-admin` (has `openshell-admin` role)
- `user1` / `user1` (has `openshell-user` + `mcp-server-a-user` roles)
- `user2` / `user2` (has `openshell-user` + `mcp-server-b-user` roles)

When the `openshell` CLI opens a browser URL for OAuth, intercept that URL and drive it through Playwright instead — fill in the username/password on the Keycloak login page, submit, and let the redirect complete back to `localhost`.

**For user onboarding (step 3):** you can use either Option A (password grant via `curl` — no browser needed) or Option B (the `onboard` tool + Playwright). Your call — whichever is more reliable.

**Definition of done:** the isolation verification in step 5 passes — user1 gets 200 from mcp-server-a and 403 from mcp-server-b, user2 gets 200 from mcp-server-b and 403 from mcp-server-a.
