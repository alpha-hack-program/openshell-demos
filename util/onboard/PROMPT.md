Build a CLI utility called `onboard` in `util/onboard/` within this repo.

## What it does

Automates the two-step user onboarding flow for OpenShell Providers v2:

1. Runs an OAuth 2.0 authorization code flow against a Keycloak instance to
   obtain an offline refresh token for a user.
2. Calls the OpenShell CLI to create a provider for that user and
   configure it with the refresh token.

OpenShell deliberately does not handle step 1 — it only manages credential
lifecycle after you hand it a refresh token. This utility bridges that gap.

## Flow

1. Build the Keycloak authorization URL:
   `https://<host>/realms/<realm>/protocol/openid-connect/auth`
   with `client_id`, `response_type=code`, `scope=openid offline_access`,
   `redirect_uri=http://127.0.0.1:<port>/callback`
2. Try to open the URL in the default browser (`xdg-open`, `open`,
   or `python -m webbrowser`). If that fails (headless, SSH), print the URL
   to stdout so the user can copy-paste it.
3. Start an HTTP listener on `127.0.0.1:<port>` waiting for Keycloak's
   callback. On success, show a clean HTML page ("Authorization code
   received, you can close this tab"). On error from Keycloak
   (`error=` param in the callback), show the error and exit non-zero.
   If no callback arrives within the timeout, exit non-zero.
4. Exchange the authorization code for tokens via a POST to
   `https://<host>/realms/<realm>/protocol/openid-connect/token`
   with `grant_type=authorization_code`. Extract the `refresh_token`.
   Verify `refresh_expires_in` is 0 (offline token) — warn if not.
5. Call the OpenShell CLI to onboard the user:
   - `openshell provider profile import -f <profile-path>` (idempotent)
   - `openshell provider create --name user-<id> --type user-scoped-api --credential USER_ACCESS_TOKEN=pending`
   - `openshell provider refresh configure user-<id> --credential-key USER_ACCESS_TOKEN --strategy oauth2-refresh-token --material client_id=<cli-client-id> --material refresh_token=<token> --secret-material-key refresh_token`
   Note: the refresh material uses the public CLI client (`openshell-cli`),
   not the confidential gateway client. Keycloak binds refresh tokens to the
   client that issued them — using a different client_id causes a 400 error.
   - `openshell provider refresh rotate user-<id> --credential-key USER_ACCESS_TOKEN`
   If any step fails, print the error and exit non-zero. If the provider
   already exists, skip creation (idempotent).

## Parameters

Required:
- `--user-id` / `-u` — user identifier (e.g. `user2`), used to
  name the provider `user-<id>`

Configurable via flags or env vars (flag takes precedence):
- `--keycloak-host` / `KEYCLOAK_HOST` — e.g. `keycloak.apps.cluster.example.com`
- `--realm` / `KEYCLOAK_REALM` — default `openshell`
- `--client-id` / `KEYCLOAK_CLIENT_ID_CLI` — the public Keycloak client for
  the browser login, default `openshell-cli`
- `--gateway-client-id` / `KEYCLOAK_CLIENT_ID_GATEWAY` — optional,
  not used in the default flow
- `--gateway-client-secret` / `KEYCLOAK_CLIENT_SECRET` — optional,
  not used in the default flow
- `--profile` — path to the provider profile YAML (required, e.g.
  `demos/keycloak-oidc/providers/user-refresh-profile.yaml`)
- `--port` — local port for the callback listener, default `9999`
- `--timeout` — seconds to wait for the callback, default `120`

Optional:
- `--token-only` — stop after step 4, print the refresh token to stdout,
  do not call OpenShell CLI (useful for scripting or debugging)
- `--no-browser` — skip the browser-open attempt, just print the URL
- `--dry-run` — print the OpenShell CLI commands that would be run in
  step 5 without executing them

## Constraints

- Language: **Rust**. This is a standalone Cargo project at `util/onboard/`
  (`util/onboard/Cargo.toml`, `util/onboard/src/main.rs`, etc.). The binary
  name should be `onboard`.
- Keep dependencies minimal but practical — suggested crates:
  `clap` (CLI args), `reqwest` (blocking HTTP client with rustls — no
  OpenSSL dependency), `serde`/`serde_json` (token response parsing),
  `open` (cross-platform browser opening), `tiny_http` or just
  `std::net::TcpListener` (callback listener). Avoid pulling in a full
  async runtime (tokio) if blocking `reqwest` + stdlib TCP suffice.
- Accept TLS certificates without verification (`danger_accept_invalid_certs`)
  to match the demo's `curl -k` everywhere pattern. Print a warning to
  stderr when this is active. Add a `--strict-tls` flag to disable this
  for production use.
- Add a thin wrapper `util/onboard/onboard.sh` that sources `.env` from the
  demo directory if it exists, exports the vars, and exec's the Rust binary
  (assuming it's been built with `cargo build --release`) — so the user can
  just `./util/onboard/onboard.sh -u user2` from anywhere.
- No secrets in code or defaults. `--gateway-client-secret` must come from
  the env or a flag, never a fallback value.
- Idempotent: safe to re-run. Provider creation should use a
  get-or-create pattern (catch "already exists" errors from the openshell
  CLI).
- Print clear status messages to stderr at each step (building URL…,
  waiting for callback…, exchanging code…, creating provider…). Only
  `--token-only` output goes to stdout.
- Read CLAUDE.md at the repo root for repo conventions before writing code.
