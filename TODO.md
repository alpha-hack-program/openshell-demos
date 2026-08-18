# TODO List

## Guthub Actions

- [x] Add a github action that compiles util/onboard and checks and add badges that proves it works
- [x] Complete the compile action with relase management and releasing bins for Linux and macOS

## General

- [x] Explain the reason for util/onboard

## Utils

- [x] Add cargo release and release management makefile as in https://github.com/cvicens/kamaji/blob/main/Makefile

## Demo Base

- [x] Smoke test should be done with Code + OpenAI API KEY

## Demo Kaycloak OIDC

- [x] Script 01-deploy-keycloak.sh should print the values to copy into demo specific .env
- [x] I want the code of this script ./scripts/02-apply-oidc-overlay.sh used directly not hidden this is a guide to learn
- [x] The demo README.md should explain that .env needs to be filled in and how
- [x] In the guide if there are two option you can choose from give a short summary before those options, oehterwise you have to read through to now what's de difference
- [x] Option B — Browser-based authorization code flow (closer to production) is superceeded by Option C, should be deleted
- [x] Add a github option that compiles util/onboard for Linux and Macos
- [x] In Step 2 there has to be a code section with the code that is run by onboard util and avoid the script, this guide is to learn and when referring to onboard util running those commands point to the code section above (when you have already added it)
- [x] The step "Before running this, substitute the real Keycloak host into providers/customer-refresh-profile.yaml'" is not well explained, I don't know what to do with this, and people running the guide without a monitor won't understand either, reword.
- [x] We don't want demo.sh I prefer commands to copy and paste and explained
- [x] For clarity move roubleshooting to a troubleshooting section to avoid clutter a guide that should follow the happy path
- [x] We should talk about users more than customers the idea is that we provide an infra so that different users with different roles can do different things for the job they have to carry out.
- [x] The demo is the important part, there should be a summary of what we're going to do and value it adds. Additionally there has to be a section in the demo summary or after it that explains the RBAC setup and the role the admin and customers play.
- [x] Rename the demos/spire demo with a proper name we don't use spire after all, suggest some names, then apply the select name to the guide and the name of the openshift project.
- [x] Upgrade `eligibility-engine-mcp-rs` (mcp-server-a) to a tag with the stateless/JSON fix — add `.with_stateful_mode(false)` before `.with_json_response(true)` in `streamable_http_config()`, same as `compatibility-engine-mcp-rs:3.1.5`. Current tag 2.0.2 is stateful/SSE which breaks curl-based testing and Codex's rmcp client.
- [x] Both MCP server images should read `MCP_STATEFUL_MODE` env var (defaults to `false`) to switch between stateful (SSE, session IDs) and stateless (plain JSON, no session) modes at runtime. Helm chart already passes the env var (`deployment.yaml`), servers just need to gate the Rust config behind it.
- [x] Both MCP servers should include the calling user's identity (extract `sub` or `preferred_username` from the Authorization Bearer JWT) in the tool response payload, making credential isolation directly visible in demo output.
- [ ] Switch Keycloak CR from edge TLS termination (`httpEnabled: true`) to re-encrypt — use an OpenShift service serving certificate so traffic is encrypted even inside the cluster
- [ ] MCP servers already return the calling user's identity (id + roles) in tool responses — update the guide to surface this: add a step or adjust existing curl/demo commands so the reader can see who invoked the tool and with which roles
- [ ] Pass a policy YAML file via `--policy` at `openshell sandbox create` time instead of requiring a separate `openshell policy update` call afterwards — the policy file can define both the endpoint and the binary permission (`network_policies` with `endpoints` + `binaries`) so sandbox creation becomes a single step. [VERIFY] Check whether the user-authored policy composes cleanly with the provider-contributed endpoint binding, or whether duplicating the endpoint causes a conflict
- [ ] Deploy EvalHub MCP server behind an Envoy sidecar gated by a new Keycloak role (`evalhub-user`) — reuses existing sidecar pattern to show AI evaluation capabilities scoped per-user identity (see EvalHub series: https://developers.redhat.com/articles/2026/05/12/how-evalhub-manages-two-layer-kubernetes-control-planes)
- [ ] Use service forwarding (`openshell service expose`) to expose something built live inside the sandbox — e.g. a web app coded by the agent, Prometheus metrics of in-sandbox work, or a dashboard — demonstrating that sandboxes are not just CLI shells but full dev environments (ref: https://docs.nvidia.com/openshell/sandboxes/manage-gateways#configure-service-forwarding)
- [ ] (Low priority) Explore Path B — SPIRE/SPIFFE token exchange as an alternative to the offline-refresh-token flow used in this demo. Was previously drafted as an "Experimental future work" section in the keycloak-oidc README; removed from the guide to keep it focused on the shipped RBAC + red-team demo, but the idea (workload identity via SPIFFE SVIDs exchanged for Keycloak tokens) may be worth prototyping separately later.
- [ ] Build a custom EvalHub BYOF adapter that red-teams Codex/Claude Code inside OpenShell sandboxes — send adversarial prompts (credential exfiltration, impersonation, unauthorized MCP server access, sandbox escape) and score whether the security layers (Keycloak tokens, Envoy RBAC, sandbox network policy, binary permissions) hold. Results flow into MLflow + OCI artifacts for repeatable, auditable evidence. Uses `FrameworkAdapter.run_benchmark_job()` from `eval-hub-sdk`. See BYOF docs: https://developers.redhat.com/articles/2026/06/09/bring-your-own-evaluation-framework-evalhub
