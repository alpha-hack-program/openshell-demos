# demos/<name> — template

Copy this file to `demos/<name>/README.md` when adding a new demo, and use
this same folder shape: `.env.example`, `helm/`, plus whatever asset folders
the demo needs (`keycloak/`, `providers/`, `policies/`, `scripts/`, etc.).
Every demo README must cover these sections:

## Purpose
What this demo shows, in 2-3 sentences.

## Prerequisites
List everything the demo needs (oc, helm, openshell CLI, etc.). Reference
[`demos/base/README.md`](../base/README.md) for shared install instructions
where useful, but don't make base a hard prerequisite — each demo should be
self-contained.

## What this demo deploys
List the Helm values it sets, and any extra infrastructure it deploys
(other Helm releases, other namespaces). Each demo carries its own complete
`helm/values.yaml` (including the OpenShift-compatibility overrides):
`helm upgrade -f demos/<name>/helm/values.yaml`.

## Architecture
Diagrams if useful (mermaid renders fine in GitHub).

## Steps
Numbered, scripted where possible, under `demos/<name>/scripts/`.

## Configuration reference
Table of env vars this demo needs. `OPENSHELL_NAMESPACE` must be in the
demo's own `.env` (not the root `.env`). Cluster-wide vars
(`OPENSHELL_CHART_VERSION`, `CLUSTER_APPS_DOMAIN`) come from the root `.env`.

## Secrets and security notes
Anything demo-specific — never rely on the reader having read another demo's notes.

## Definition of done
Checklist specific to this demo.

## Open risks / things to verify
Be explicit about anything inferred rather than confirmed against a live
source. Don't let confidence in the prose outrun confidence in the source.

## References
Links specific to this demo's technology (not repeated from root `CLAUDE.md`).

## Conventions shared across all demos

- Each demo carries its own `helm/values.yaml` with OpenShift-compatibility
  overrides included — no dependency on another demo's values file.
- Each demo defines `OPENSHELL_NAMESPACE` in its own `.env.example`.
  Use the pattern `openshell-<demo-name>-demo` (e.g. `openshell-base-demo`,
  `openshell-keycloak-oidc-demo`).
- Script filenames are numbered in run order (`01-`, `02-`, ...).
- Secrets: `.env.example` lists variable names only; real values go in a
  gitignored `.env` or a secret manager, never committed.
- If two demos would need contradictory gateway settings to coexist on one
  cluster, say so explicitly in both READMEs rather than silently overwriting.
