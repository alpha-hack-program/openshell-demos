# OpenShell on OpenShift — demos

[![OpenShell](https://img.shields.io/badge/OpenShell_Chart-0.0.106-blue)](https://github.com/NVIDIA/OpenShell)
[![CI onboard CLI](https://github.com/alpha-hack-program/openshell-demos/actions/workflows/ci-onboard.yml/badge.svg)](https://github.com/alpha-hack-program/openshell-demos/actions/workflows/ci-onboard.yml)
[![CI agent-proxy](https://github.com/alpha-hack-program/openshell-demos/actions/workflows/ci-agent-proxy.yml/badge.svg)](https://github.com/alpha-hack-program/openshell-demos/actions/workflows/ci-agent-proxy.yml)

> **Experimental — not production-ready.**
> These demos build on top of the
> [official OpenShell on OpenShift guide](https://docs.nvidia.com/openshell/kubernetes/openshift)
> but diverge from it in significant ways (OIDC integration, per-user
> credential isolation, MCP server gating). The goal is to show how
> OpenShell can help you adopt **secure, isolated execution of AI agents on
> OpenShift** — and to give you a head start exploring that path. That said,
> the patterns here are still evolving and should not be treated as
> production-grade. Use them to learn, experiment, and evaluate — then
> follow the official documentation when you're ready to deploy for real.

Everything lives under `demos/`. Each subfolder is a self-contained demo with
its own README, Helm values, and extra infrastructure. Every demo uses the
same Helm release name (`openshell`) but deploys into its **own namespace**
(`OPENSHELL_NAMESPACE` in each demo's `.env`) — the namespace is what keeps
demos isolated from each other on the same cluster.

Start with [`demos/base/README.md`](demos/base/README.md) for a from-scratch
install. Once that's green, pick another demo.

| Demo | What it covers | Status |
|---|---|---|
| [`base`](demos/base/README.md) | Demo-agnostic OpenShell gateway install + hello-world sandbox verification, plus an optional DeepSeek credential-injection smoke test | Verified end to end |
| [`keycloak-oidc`](demos/keycloak-oidc/README.md) | Keycloak as OIDC IdP, per-user credential isolation via Providers v2, MCP servers gated by Keycloak role via an Envoy sidecar | Verified end to end (real LLM + real MCP tool calls, cross-user/cross-server isolation confirmed) |
| [`saw-openclaw`](demos/saw-openclaw/README.md) | SAW (Secure Agent Workspace) with KubeVirt VM isolation, OpenClaw running Gemini inference inside a sandboxed container, includes workarounds for current SAW deployment issues | Verified end to end (inference, code generation, reasoning) |

## Documentation

Cross-cutting guides and reference material that apply across demos:

| Document | What it covers |
|---|---|
| [`docs/sandbox-service-patterns.md`](docs/sandbox-service-patterns.md) | Custom images, static binaries, background services, `service expose` vs `--forward`, toolbox workarounds |
| [`docs/inference-api-compatibility.md`](docs/inference-api-compatibility.md) | Which LLM API formats each agent requires, provider compatibility matrix, test scripts |
| [`docs/openshell-flows.md`](docs/openshell-flows.md) | Operational flows by role (admin vs user) and auth mode (mTLS vs OIDC), with diagrams |
| [`docs/headless-browser-automation.md`](docs/headless-browser-automation.md) | Playwright setup for automating OAuth flows in headless / CI environments |
| [`docs/guide-testing-protocol.md`](docs/guide-testing-protocol.md) | Methodology for testing a demo guide end to end: follow every step literally, handle failures, resolve `[VERIFY]` tags |
| [`demos/keycloak-oidc/docs/evalhub-redteam.md`](demos/keycloak-oidc/docs/evalhub-redteam.md) | **DRAFT** — red-team evaluations with EvalHub + Garak + agent-proxy inside sandboxes (part of the keycloak-oidc demo) |

See [`CLAUDE.md`](CLAUDE.md) for the full build contract, conventions, and notes
for Claude Code working in this repo.

Have fun !
