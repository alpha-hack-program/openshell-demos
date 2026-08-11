# OpenShell on OpenShift — demos

[![OpenShell](https://img.shields.io/badge/OpenShell_Chart-0.0.101-blue)](https://github.com/NVIDIA/OpenShell)
[![Build onboard CLI](https://github.com/alpha-hack-program/openshell-demos/actions/workflows/build-onboard.yml/badge.svg)](https://github.com/alpha-hack-program/openshell-demos/actions/workflows/build-onboard.yml)

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

See [`CLAUDE.md`](CLAUDE.md) for the full build contract, conventions, and notes
for Claude Code working in this repo.

Have fun !
