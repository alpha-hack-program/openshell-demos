# CLAUDE.md — OpenShell on OpenShift: base install + demos

## How to use this file

This is the orientation document for Claude Code working in this repo. It
covers repo shape, the contract between `base/` and `demos/`, and cross-repo
conventions. It deliberately does **not** duplicate installation steps —
those live in [`base/README.md`](base/README.md) and each
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

## 1. Repo shape and the base/demos contract

```
.
├── CLAUDE.md              # this file
├── README.md              # human-facing repo overview + demo index
├── .env.example            # cluster-wide variables
├── base/                   # demo-agnostic OpenShell-on-OpenShift install
│   ├── README.md
│   ├── helm/values-openshift.yaml
│   └── scripts/
└── demos/
    ├── _template/README.md # copy this when adding a new demo
    └── spire-spiffe-keycloak/
        ├── README.md
        ├── helm/values-overlay.yaml
        ├── keycloak/ | spire/ | providers/ | policies/ | mcp-servers/
        └── scripts/
```

- **`base/` is demo-agnostic.** It installs a working OpenShell gateway on
  OpenShift and proves it with a generic hello-world sandbox (no OIDC, no
  SPIFFE, no external identity provider). Nothing under `base/` should ever
  assume any particular demo. Changes to `base/` should make sense even if
  every demo folder were deleted.
- **Each `demos/<name>/` is a self-contained, independent OpenShell
  install.** Own README, own Helm values overlay, own scripts, own extra
  infrastructure (Keycloak, SPIRE, whatever it needs), own provider profiles
  and policies, and — critically — its **own namespace**, independent of
  whatever namespace `base/`'s own install uses on the same cluster. A demo
  never edits `base/helm/values-openshift.yaml` directly — it supplies a
  second `-f` file, applied on top, targeting its own `OPENSHELL_NAMESPACE`:
  ```bash
  helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
    --version "$OPENSHELL_CHART_VERSION" --namespace "$OPENSHELL_NAMESPACE" \
    -f base/helm/values-openshift.yaml \
    -f demos/<name>/helm/values-overlay.yaml
  ```
  Check a demo's own `.env`/README for which namespace it actually targets
  — don't assume it matches `base/`'s. The gateway, Route, and everything
  else that command creates belongs entirely to that demo, not to `base/`'s
  own running install.
- **Order of operations:** finish `base/`'s Definition of Done first — this
  proves the chart/cluster combination works at all — before starting a
  demo. A demo's own README states its prerequisites beyond `base/`
  explicitly — don't assume.
- **Multiple demos coexist trivially** on one cluster, since each deploys
  into its own namespace with its own gateway release — there's no shared
  gateway config to conflict over.

## 2. Adding a new demo

Copy [`demos/_template/README.md`](demos/_template/README.md) to
`demos/<name>/README.md` and keep its section headings — Purpose,
Prerequisites beyond base, What this demo adds on top of base, Architecture,
Steps, Configuration reference, Secrets and security notes, Definition of
done, Open risks, References. Use a short descriptive name for the folder
(e.g. `spire-spiffe-keycloak`), **not** a numeric prefix — numerals belong
on scripts *inside* a demo folder (`00-prereqs.sh`, `01-deploy.sh`, …)
where they reflect execution order, not on the folder itself.

## 3. Cross-repo conventions

- **Secrets:** every `.env.example` lists variable names only. Real values go
  in a gitignored `.env` (or a secret manager) at the same level, never
  committed. Realm exports / client configs use `.template.` filenames with
  placeholder values, substituted at deploy time by a script.
- **Script numbering:** `00-`, `01-`, `02-`, ... reflects run order within a
  folder.
- **Idempotency:** scripts should be safe to re-run (`get || create` patterns,
  `helm upgrade --install` rather than bare `install`).
- **Honesty about confidence:** if a command in a README is inferred rather
  than confirmed against a live source (docs, `--help` output, an actual
  running cluster), it's marked **[VERIFY]** in that README and stays marked
  until someone actually checks it — don't quietly drop the tag.

## 4. Demos in this repo

| Name | Adds on top of base | Status |
|---|---|---|
| [`spire-spiffe-keycloak`](demos/spire-spiffe-keycloak/README.md) | Keycloak as OIDC IdP, per-customer dynamic credentials (Providers v2 refresh strategy), MCP servers gated by Keycloak role via an Envoy sidecar, and — as a stretch goal — SPIFFE/SPIRE token-exchange grants matching NVIDIA's own `spiffe-token-grant-demo` | Keycloak/Providers-v2 path and the MCP servers extension verified end to end against a live cluster; SPIRE/SPIFFE has never actually been deployed — see the demo's Open Risks |

## 5. References (repo-wide)

Demo-specific links live in each demo's own README. These apply across the
whole repo:

- OpenShell repo: https://github.com/NVIDIA/OpenShell
- OpenShell docs home: https://docs.nvidia.com/openshell
- OpenShift install path: https://docs.nvidia.com/openshell/kubernetes/openshift
- Support matrix: https://docs.nvidia.com/openshell/reference/support-matrix
