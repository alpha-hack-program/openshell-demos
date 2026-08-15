# CLAUDE.md — OpenShell on OpenShift: base install + demos

## How to use this file

This is the orientation document for Claude Code working in this repo. It
covers repo shape, the conventions shared across demos, and cross-repo
conventions. It deliberately does **not** duplicate installation steps —
those live in [`demos/base/README.md`](demos/base/README.md) and each
`demos/<name>/README.md`. Read this file first, then the relevant README for
whatever you're actually building.

Before running anything against a real cluster:

1. **Check tool versions.** Run `openshell --version`, `helm show values
   oci://ghcr.io/nvidia/openshell/helm-chart`, etc. before assuming any flag
   in a README still matches — this space moves fast.
2. **Don't apply anything without confirming with the user first** — this
   includes `oc adm policy add-scc-to-user`, `helm install`/`upgrade`, and
   anything that creates Keycloak clients, secrets, or other cluster state.
   Scaffold and show the plan, then ask.
3. **When a README says [VERIFY], verify it** — don't silently promote an
   inferred command to a confirmed one just because it's already in the file.

## 1. Repo shape

```
.
├── CLAUDE.md              # this file
├── README.md              # human-facing repo overview + demo index
├── .env.example            # cluster-wide variables
└── demos/
    ├── _template/README.md # copy this when adding a new demo
    ├── base/               # demo-agnostic OpenShell-on-OpenShift install
    │   ├── README.md
    │   ├── .env.example
    │   ├── helm/values-openshift.yaml
    │   └── scripts/
    └── keycloak-oidc/
        ├── README.md
        ├── .env.example
        ├── helm/values.yaml
        ├── keycloak/ | providers/ | policies/ | mcp-servers/
        └── scripts/
```

- **`demos/base/` is the foundational demo.** It installs a working OpenShell
  gateway on OpenShift and proves it with a generic hello-world sandbox (no
  OIDC, no SPIFFE, no external identity provider). Nothing under `demos/base/`
  should ever assume any particular other demo. Changes to `demos/base/`
  should make sense even if every other demo folder were deleted.
- **Each `demos/<name>/` is a self-contained, independent OpenShell
  install.** Own README, own `.env` / `.env.example`, own Helm values file,
  own scripts, own extra infrastructure (Keycloak, whatever it
  needs), own provider profiles and policies, and — critically — its **own
  namespace** via `OPENSHELL_NAMESPACE` in its own `.env`. Namespace is
  always per-demo, never shared — it does not live in the root `.env`.
  Each demo carries a complete `helm/values.yaml` (including the
  OpenShift-compatibility overrides) so it can be installed with a single
  `-f`:
  ```bash
  helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
    --version "$OPENSHELL_CHART_VERSION" --namespace "$OPENSHELL_NAMESPACE" \
    -f demos/<name>/helm/values.yaml
  ```
  The root `.env` holds only cluster-wide variables
  (`OPENSHELL_CHART_VERSION`, `CLUSTER_APPS_DOMAIN`). Check a demo's own
  `.env`/README for which namespace it targets.
- **Order of operations:** `demos/base/` is a good first demo — it proves
  the chart/cluster combination works at all — but each demo is meant to
  stand on its own. A demo's own README states its prerequisites
  explicitly — don't assume.
- **Multiple demos coexist trivially** on one cluster, since each deploys
  into its own namespace with its own gateway release — there's no shared
  gateway config to conflict over.

## 2. Adding a new demo

Copy [`demos/_template/README.md`](demos/_template/README.md) to
`demos/<name>/README.md` and keep its section headings — Purpose,
Prerequisites, What this demo deploys, Architecture,
Steps, Configuration reference, Secrets and security notes, Definition of
done, Open risks, References. Use a short descriptive name for the folder
(e.g. `keycloak-oidc`), **not** a numeric prefix — numerals belong
on scripts *inside* a demo folder (`00-prereqs.sh`, `01-deploy.sh`, …)
where they reflect execution order, not on the folder itself.
Do **not** place new demos inside `demos/base/` — `base` is a peer demo, not
a parent directory for others.

## 3. Cross-repo conventions

- **Environment variables:** the root `.env` holds cluster-wide variables
  (`OPENSHELL_CHART_VERSION`, `CLUSTER_APPS_DOMAIN`). Each demo has its own
  `.env` with demo-specific variables — at minimum `OPENSHELL_NAMESPACE`.
  Every `.env.example` lists variable names only. Real values go in a
  gitignored `.env` (or a secret manager) at the same level, never
  committed. Realm exports use hardcoded demo-only credentials (see each
  demo's README for details); in production, generate unique secrets per
  environment.
- **Script numbering:** `00-`, `01-`, `02-`, ... reflects run order within a
  folder.
- **Idempotency:** scripts should be safe to re-run (`get || create` patterns,
  `helm upgrade --install` rather than bare `install`).
- **Honesty about confidence:** if a command in a README is inferred rather
  than confirmed against a live source (docs, `--help` output, an actual
  running cluster), it's marked **[VERIFY]** in that README and stays marked
  until someone actually checks it — don't quietly drop the tag.

## 4. Demos in this repo

| Name | What it covers | Status |
|---|---|---|
| [`base`](demos/base/README.md) | Demo-agnostic OpenShell-on-OpenShift install + hello-world sandbox verification | Verified end to end |
| [`keycloak-oidc`](demos/keycloak-oidc/README.md) | Keycloak as OIDC IdP, per-user credential isolation via Providers v2, MCP servers gated by Keycloak role via an Envoy sidecar | Verified end to end against a live cluster |

## 5. Running demos headlessly (Claude-specific)

When running demo guides that involve browser-based OAuth flows (e.g.
`openshell gateway login`, the `onboard` tool), use **Playwright** with
headless Chromium to automate the Keycloak login form. This section
captures the quirks discovered while running the guides end to end.

### Playwright setup

```bash
mkdir -p /tmp/playwright-scratch && cd /tmp/playwright-scratch
npm init -y && npm install playwright
npx playwright install chromium
```

### Keycloak demo credentials

Demo user credentials (usernames, passwords, role assignments) are defined
in each demo's realm export JSON — e.g.
`demos/keycloak-oidc/keycloak/realm-export.json`. Parse that file for
usernames and passwords rather than hardcoding them.

### Preventing real browser popups

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

### Browser-based OAuth flows

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

### Keycloak login form selectors

The Keycloak login page uses these selectors (stable across Keycloak 26+):
- Username: `#username`
- Password: `#password`
- Submit button: `#kc-login`

After submitting, wait for the redirect to `localhost` (the OAuth callback).

### OpenShell CLI quirks

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

## 6. References (repo-wide)

Demo-specific links live in each demo's own README. These apply across the
whole repo:

- OpenShell repo: https://github.com/NVIDIA/OpenShell
- OpenShell docs home: https://docs.nvidia.com/openshell
- OpenShift install path: https://docs.nvidia.com/openshell/kubernetes/openshift
- Support matrix: https://docs.nvidia.com/openshell/reference/support-matrix
