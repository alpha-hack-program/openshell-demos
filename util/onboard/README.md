# onboard — OpenShell user-onboarding CLI

## Why this tool exists

OpenShell's Providers v2 manages credential lifecycle — rotation, injection,
scoping — but it deliberately stays out of the *initial authentication* step.
It expects you to hand it a refresh token; it does not help you obtain one.

On the Keycloak side, getting an offline refresh token means running a
browser-based OAuth 2.0 authorization code flow (redirect, consent, code
exchange). That flow involves a local HTTP callback listener, URL construction,
token parsing, and offline-token verification — none of which the OpenShell CLI
covers.

`onboard` bridges that gap. It handles the full sequence in a single command:

1. Builds the Keycloak authorization URL and opens the browser.
2. Listens on a local port for the OAuth callback.
3. Exchanges the authorization code for an offline refresh token.
4. Calls the OpenShell CLI to create a per-user provider and configure it
   with the token.

Without it, onboarding each user requires copy-pasting five or six commands
across two systems (Keycloak token endpoint + OpenShell CLI), manually
extracting the refresh token from a JSON response, and verifying that it's
actually an offline token (`refresh_expires_in=0`). The tool makes this
repeatable and less error-prone.

## Quick start

```bash
# Build
make release

# Install to ~/bin (optional)
make install

# Onboard a user (reads KEYCLOAK_HOST etc. from the demo's .env)
./onboard.sh -u user2 --profile ../../demos/keycloak-oidc/providers/user-refresh-profile.yaml
```

Run `onboard --help` for all flags, or see the
[keycloak-oidc demo README](../../demos/keycloak-oidc/README.md) for the full
walkthrough.

## Development

```bash
make help       # list all targets
make check      # fmt-check + clippy + tests
make run ARGS="--help"
```

## Releasing

Requires [`cargo-release`](https://github.com/crate-ci/cargo-release):

```bash
cargo install cargo-release
make release-patch   # or release-minor / release-major
```

This bumps `Cargo.toml`, runs `make ci` as a gate, commits, tags `v<version>`,
and pushes. GitHub Actions then builds Linux and macOS binaries and creates a
GitHub Release.
