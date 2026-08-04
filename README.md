# OpenShell on OpenShift — base install + demos

This repo has two layers:

- **`base/`** — demo-agnostic. Installs the OpenShell gateway on OpenShift and
  proves it works with a generic hello-world sandbox. Nothing here assumes any
  particular demo.
- **`demos/`** — each subfolder is a self-contained demo that builds on top of a
  working `base/` install: its own README, its own Helm values overlay, its own
  extra infrastructure and assets.

Start with [`base/README.md`](base/README.md). Once that's green, pick a demo.

| Demo | Adds on top of base | Status |
|---|---|---|
| [`spire-spiffe-keycloak`](demos/spire-spiffe-keycloak/README.md) | Keycloak as OIDC IdP, per-customer dynamic credentials, SPIFFE/SPIRE token-exchange grants | Draft — see its README for open risks |

See [`CLAUDE.md`](CLAUDE.md) for the full build contract, conventions, and notes
for Claude Code working in this repo.
