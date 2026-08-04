# demos/<NN-name> — template

Copy this file to `demos/<NN-name>/README.md` when adding a new demo, and use
this same folder shape: `helm/`, plus whatever asset folders the demo needs
(`keycloak/`, `providers/`, `policies/`, `scripts/`, etc.). Every demo README
must cover these sections:

## Purpose
What this demo shows, in 2-3 sentences, and why it's interesting beyond `base/`.

## Prerequisites beyond base
Anything not already covered by `base/README.md`'s prerequisites.

## What this demo adds on top of base
List the Helm overlay keys it sets, and any extra infrastructure it deploys
(other Helm releases, other namespaces). Be explicit that the overlay is
applied additively: `helm upgrade -f base/helm/values-openshift.yaml -f demos/<name>/helm/values-overlay.yaml`.

## Architecture
Diagrams if useful (mermaid renders fine in GitHub).

## Steps
Numbered, scripted where possible, under `demos/<name>/scripts/`.

## Configuration reference
Table of env vars this demo needs, beyond the root `.env`.

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

- A demo's Helm overlay only ever *adds to or overrides* `base/helm/values-openshift.yaml`
  — it never requires editing `base/`.
- Script filenames are numbered in run order (`01-`, `02-`, ...).
- Secrets: `.env.example` lists variable names only; real values go in a
  gitignored `.env` or a secret manager, never committed.
- If two demos would need contradictory gateway settings to coexist on one
  cluster, say so explicitly in both READMEs rather than silently overwriting.
