# AGENTS.md — OpenShell on OpenShift: base install + demos

## How to use this file

This is the orientation document for any AI agent working in this repo. It
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
├── CLAUDE.md              # agent entry point (references this file)
├── AGENTS.md              # this file — repo orientation + conventions
├── README.md              # human-facing repo overview + demo index
├── .env.example           # cluster-wide variables
├── docs/
│   ├── headless-browser-automation.md  # Playwright setup + OAuth flow automation
│   ├── guide-testing-protocol.md       # how to test a demo guide end to end
│   ├── openshell-flows.md
│   └── diagrams/
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
  **Namespace names must not start with `openshell-`.** At least one demo
  derives its gateway Route hostname as
  `openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}`, and a
  redundant prefix can push the FQDN over the 64-byte X.509 CommonName
  limit when using an external ACME issuer (e.g. Let's Encrypt) for the
  Route cert. Use a short demo slug alone, e.g. `keycloak-oidc-demo`, not
  `openshell-keycloak-oidc-demo`.
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

- **Branch naming:** version-update branches use `v<VERSION>` (e.g.
  `v0.0.106`). Feature or fix branches use a short descriptive slug.
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
- **Keycloak operator:** always use the **Red Hat build of Keycloak**
  (`rhbk-operator` from the **Red Hat Operators** catalog). Search for
  `rhbk`, not `keycloak` — searching `keycloak` returns unrelated community
  operators. Never suggest the community `keycloak-operator` or other
  alternatives without first confirming `rhbk-operator` is unavailable via
  `oc get packagemanifests -n openshift-marketplace | grep rhbk-operator`.
- **Honesty about confidence:** if a command in a README is inferred rather
  than confirmed against a live source (docs, `--help` output, an actual
  running cluster), it's marked **[VERIFY]** in that README and stays marked
  until someone actually checks it — don't quietly drop the tag.

## 4. Running demos headlessly

When running demos without a GUI (e.g. from a CLI agent), browser-based
OAuth flows need to be automated. Two docs cover this:

- **[Headless browser automation](docs/headless-browser-automation.md)** —
  Playwright setup, xdg-open interception, Keycloak form selectors,
  CLI + Playwright orchestration pattern, and OpenShell CLI quirks.
- **[Guide testing protocol](docs/guide-testing-protocol.md)** — the
  methodology for testing a demo guide end to end: follow every step
  literally, handle failures, resolve `[VERIFY]` tags, and report results.
- **`openshell settings set --global`** requires `--yes` in non-interactive
  mode (the interactive prompt can't be answered by an agent). Append
  `--yes` when running headlessly.
- **keycloak-oidc test scope:** headless testing of `demos/keycloak-oidc`
  must always run the main recipe (steps 1–5, including the curl-based
  isolation verification). The README also contains optional recipes
  (Codex + BYO LLM, Claude Code + BYO LLM) — ask the user whether to
  run them before finishing. Never skip the optional recipes silently:
  either run them or explicitly report that they were not tested.

## 5. Demos in this repo

| Name | What it covers | Status |
|---|---|---|
| [`base`](demos/base/README.md) | Demo-agnostic OpenShell-on-OpenShift install + hello-world sandbox verification | Verified end to end |
| [`keycloak-oidc`](demos/keycloak-oidc/README.md) | Keycloak as OIDC IdP, per-user credential isolation via Providers v2, MCP servers gated by Keycloak role via an Envoy sidecar | Verified end to end against a live cluster |

## 6. Internal documentation (`docs/`)

Read these before working on the relevant area — they capture patterns
and constraints that aren't obvious from the code alone:

- **[Sandbox service patterns](docs/sandbox-service-patterns.md)** —
  custom images (static binaries, Containerfile layout, remote gateway
  build+push workflow), running background services inside sandboxes,
  `service expose` vs `--forward`, Host-header routing, toolbox
  workarounds. Start here for anything involving long-running processes
  in sandboxes (agent-proxy, Prometheus exporters, dev servers).
- **[Inference API compatibility](docs/inference-api-compatibility.md)** —
  which LLM API format each agent requires (Codex → OpenAI Responses API
  with namespace tools; Claude Code → Anthropic Messages API), provider
  compatibility matrix, vLLM version requirements, and a test script.
- **[OpenShell flows](docs/openshell-flows.md)** — operational flows
  by role (admin vs user) and auth mode (mTLS vs OIDC), with SVG
  diagrams. Useful for understanding the end-to-end lifecycle before
  writing new demo steps.
- **[Headless browser automation](docs/headless-browser-automation.md)** —
  Playwright setup, xdg-open interception, Keycloak form selectors,
  CLI + Playwright orchestration pattern, and OpenShell CLI quirks.
- **[Guide testing protocol](docs/guide-testing-protocol.md)** — how to
  test a demo guide end to end: follow every step literally, handle
  failures, resolve `[VERIFY]` tags, and report results.
- **[EvalHub red-team plan](demos/keycloak-oidc/docs/evalhub-redteam.md)** —
  **DRAFT.** Agent-proxy + Garak + EvalHub for red-team evaluations
  inside sandboxes. Contains resolved design decisions, validated
  lifecycle findings, and open items. Lives under `demos/keycloak-oidc/`
  because the demo extends that stack.
- **[Self-service onboarding design notes](demos/keycloak-oidc/docs/self-service-onboarding.md)** —
  **DRAFT, brainstorm only.** Explores replacing the operator-run `onboard`
  CLI with a self-service web app. Covers why every provisioning call
  requires Platform Admin today, a security-first analysis (authentication
  vs. authorization to provision, JML/approval patterns, standing-credential
  risk), architecture options, and a recommendation. No implementation yet.

## 7. References (repo-wide)

Demo-specific links live in each demo's own README. These apply across the
whole repo:

- OpenShell repo: https://github.com/NVIDIA/OpenShell
- OpenShell docs home: https://docs.nvidia.com/openshell
- OpenShift install path: https://docs.nvidia.com/openshell/kubernetes/openshift
- Support matrix: https://docs.nvidia.com/openshell/reference/support-matrix
