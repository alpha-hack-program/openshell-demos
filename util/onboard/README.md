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

## Installation

### From GitHub Releases (recommended)

Download the latest binary for your platform from the
[Releases page](https://github.com/alpha-hack-program/openshell-demos/releases):

```bash
# Linux (x86_64)
curl -fsSL -o onboard \
  https://github.com/alpha-hack-program/openshell-demos/releases/latest/download/onboard-linux-x86_64
chmod +x onboard
sudo install onboard /usr/local/bin/

# macOS (Apple Silicon)
curl -fsSL -o onboard \
  https://github.com/alpha-hack-program/openshell-demos/releases/latest/download/onboard-macos-aarch64
chmod +x onboard
install onboard ~/bin/
```

### From source

Requires Rust 2024 edition (1.85+):

```bash
cd util/onboard
make release          # builds target/release/onboard
make install          # copies to ~/bin/onboard
```

Or directly with cargo:

```bash
cargo build --release --manifest-path util/onboard/Cargo.toml
```

## Usage

```
onboard [OPTIONS] -u <USER_ID> --profile <PROFILE>
```

The tool reads Keycloak connection settings from flags or environment variables.
The included `onboard.sh` wrapper sources `demos/keycloak-oidc/.env`
automatically, so you can skip the `--keycloak-host` / env setup if that file
is populated:

```bash
./onboard.sh -u user2 --profile ../../demos/keycloak-oidc/providers/user-refresh-profile.yaml
```

### Flags

| Flag | Env var | Default | Description |
|---|---|---|---|
| `-u`, `--user-id` | — | *required* | User identifier (e.g. `user2`); provider will be named `user-<id>` |
| `--keycloak-host` | `KEYCLOAK_HOST` | *required* | Keycloak hostname (e.g. `keycloak.apps.cluster.example.com`) |
| `--realm` | `KEYCLOAK_REALM` | `openshell` | Keycloak realm |
| `--client-id` | `KEYCLOAK_CLIENT_ID_CLI` | `openshell-cli` | Public Keycloak client for the browser login |
| `--gateway-client-id` | `KEYCLOAK_CLIENT_ID_GATEWAY` | `openshell-gateway` | Confidential client (not needed for public-client flow) |
| `--gateway-client-secret` | `KEYCLOAK_CLIENT_SECRET` | — | Confidential client secret (not needed for public-client flow) |
| `--profile` | — | *required* | Path to the provider profile YAML |
| `--namespace` | `OPENSHELL_NAMESPACE` | — | Substituted for `<openshell-namespace>` in the profile, if present. `onboard.sh` sources this from `.env` automatically |
| `--workspace` | `OPENSHELL_WORKSPACE` | the user ID | OpenShell workspace to create the provider in. Each onboarded user should have their **own** workspace — putting multiple users in one shared workspace defeats isolation (a `user`-role member of a workspace can see and act on *every* sandbox in that workspace, not just their own; see [Manage Workspaces and Access](https://docs.nvidia.com/openshell/sandboxes/manage-workspaces)). The tool does not create the workspace or grant membership — a platform admin must have already run `openshell workspace create` and `openshell workspace member add` for this user first |
| `--port` | — | `9999` | Local port for the OAuth callback listener |
| `--timeout` | — | `120` | Seconds to wait for the callback |
| `--token-only` | — | — | Stop after obtaining the refresh token; print it to stdout |
| `--no-browser` | — | — | Skip browser-open attempt; just print the URL |
| `--dry-run` | — | — | Print OpenShell CLI commands without executing them |
| `-v`, `--verbose` | — | — | Show openshell commands and substitutions as they run |
| `--strict-tls` | — | — | Require valid TLS certificates (default: accept self-signed) |

### Examples

Onboard a user against a live cluster (env vars from `.env`):

```bash
export KEYCLOAK_HOST=keycloak.apps.mycluster.example.com
onboard -u user3 --profile demos/keycloak-oidc/providers/user-refresh-profile.yaml
```

Preview what OpenShell commands would run without executing them:

```bash
onboard -u user3 --profile path/to/profile.yaml --dry-run -v
```

Get just the offline refresh token (e.g. for scripting):

```bash
TOKEN=$(onboard -u user3 --profile path/to/profile.yaml --token-only)
```

Headless / SSH session — print the URL instead of opening a browser:

```bash
onboard -u user3 --profile path/to/profile.yaml --no-browser
```

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

This bumps `Cargo.toml`, runs `make ci` as a gate, commits, tags
`onboard-v<version>`, and pushes. GitHub Actions then builds Linux and macOS
binaries and creates a GitHub Release with the attached artifacts.
