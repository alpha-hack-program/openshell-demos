# OpenShell on OpenShift — base install + demos

This repo has two layers:

- **`base/`** — demo-agnostic. Installs the OpenShell gateway on OpenShift and
  proves it works with a generic hello-world sandbox, plus an optional
  DeepSeek credential-injection smoke test. Nothing here assumes any
  particular demo.
- **`demos/`** — each subfolder is a self-contained demo with its own README,
  Helm values overlay, and extra infrastructure. A demo deploys into its
  **own namespace**, independent of whatever namespace `base/`'s own install
  uses on the same cluster — check a demo's own `.env`/README for which
  namespace it actually targets.

Start with [`base/README.md`](base/README.md) for a from-scratch install.
Once that's green, pick a demo.

| Demo | Adds on top of base | Status |
|---|---|---|
| [`spire-spiffe-keycloak`](demos/spire-spiffe-keycloak/README.md) | Keycloak as OIDC IdP, per-customer dynamic credentials (Path A), MCP servers gated by Keycloak role via an Envoy sidecar, SPIFFE/SPIRE token-exchange grants (Path B, stretch) | Path A and the MCP servers extension verified end to end (real LLM + real MCP tool calls, cross-customer/cross-server isolation confirmed); Path B (SPIRE) has never actually been deployed — see its README's Open Risks |

See [`CLAUDE.md`](CLAUDE.md) for the full build contract, conventions, and notes
for Claude Code working in this repo.
