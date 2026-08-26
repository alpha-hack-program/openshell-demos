I need you to run the keycloak-oidc demo from `demos/keycloak-oidc/README.md` end to end, playing four roles: **admin**, **alice**, **bob**, and **charlie**.

**Important constraints:**

- **Don't change any files in the repo** — no edits to READMEs, scripts, helm values, etc.
- **Confirm with me before running any cluster-mutating command** (`oc apply`, `helm install/upgrade`, `oc adm policy`, secret creation, etc.) — show me what you're about to run and wait for my go-ahead.
- Follow the README steps in order (1 through 5), including the MCP server deployment (step 4) and the isolation verification (step 5).

**Browser-based OAuth flows:**

For any step that requires a browser login (e.g. `openshell gateway login`, the `onboard` tool's OAuth callback), use **Playwright** with headless Chromium to automate the Keycloak login form. Playwright is installed at `/tmp/playwright-scratch/` (run scripts with `cd /tmp/playwright-scratch && node -e "..."` or `node /path/to/script.js`). The Chromium binary is cached in `~/.cache/ms-playwright/`.

The Keycloak demo bankers and their passwords (from `keycloak/realm-export.json`):
- `openshell-admin` / `openshell-admin` (has `openshell-admin` role)
- `alice` / `alice` (has `openshell-user` + `banker` roles, plus `compatibility-user` via the `compatibility-users` group)
- `bob` / `bob` (has `openshell-user` + `banker` roles)
- `charlie` / `charlie` (has `openshell-user` + `banker` roles)

When the `openshell` CLI opens a browser URL for OAuth, intercept that URL and drive it through Playwright instead — fill in the username/password on the Keycloak login page, submit, and let the redirect complete back to `localhost`.

**For banker onboarding (step 3):** you can use either Option A (password grant via `curl` — no browser needed) or Option B (the `onboard` tool + Playwright). Your call — whichever is more reliable. Onboard all three bankers.

**Definition of done:** the isolation verification in step 5 passes for all three bankers across all four MCP servers — alice gets 200 from mcp-compatibility/mcp-portfolio/mcp-crm-calendar/mcp-market-news; bob and charlie each get 403 from mcp-compatibility and 200 from the other three; and Bob's cross-tenant probe against Alice's and Charlie's `client_id`s via `mcp-portfolio`'s `get_positions` is denied with the same ambiguous error a nonexistent `client_id` gets.
