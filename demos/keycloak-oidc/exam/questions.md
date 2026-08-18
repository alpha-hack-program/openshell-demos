# OpenShell + Keycloak OIDC + Agent Integration — Exam

**Pass mark:** 16 / 20 (80 %)
**Scope:** Admin and user workflows for OpenShell on OpenShift with Keycloak OIDC, credential isolation, MCP servers, and AI-agent integration (Codex, Claude Code).

---

### Q1 — Supervisor credential handling

The supervisor runs inside the sandbox pod. Describe **how** it prevents real credentials from ever entering the sandbox process, and explain what the sandbox process sees instead of the actual secret.

---

### Q2 — mTLS + OIDC dual authentication

When OIDC is enabled, OpenShell enforces **two** authentication layers on every gRPC call. Name both layers, state what each one validates, and explain what Helm value must change (compared to the base install) to activate the OIDC layer.

---

### Q3 — Keycloak operator choice

Which Keycloak operator must be used on OpenShift, and from which operator catalog? Why is this choice explicit in the project conventions, and how would you verify its availability on the cluster?

---

### Q4 — Provider profile vs. provider instance

Explain the difference between a **provider profile** and a **provider instance**. Give one concrete example of each from the keycloak-oidc demo.

---

### Q5 — Default sandbox network posture

What is the default outbound network posture of an OpenShell sandbox? How does an admin open access to a specific external endpoint, and what two dimensions must be specified together in a policy rule?

---

### Q6 — Envoy sidecar as the sole enforcement point

The keycloak-oidc demo deploys MCP servers with an Envoy sidecar. Describe the defense-in-depth layers that protect an MCP server, and explain why removing the Envoy sidecar would be a critical security failure — what was observed when the MCP app was tested directly with no token, a garbage token, and a valid token lacking the required role?

---

### Q7 — OIDC login trigger

A new user runs `openshell gateway add` with the OIDC flags (`--oidc-issuer`, `--oidc-client-id`, `--oidc-scopes`). Does the user need to run a separate `openshell gateway login` command afterwards? Explain what happens during `gateway add`.

---

### Q8 — Resolve-placeholder lifecycle

Trace the full lifecycle of a credential from the gateway to the upstream API call, covering: (a) how the gateway delivers the credential, (b) what the sandbox env var contains, (c) how the application code uses it, and (d) what the supervisor does when the outbound request leaves the sandbox.

---

### Q9 — Endpoint binding (0.0.106+)

Starting with OpenShell 0.0.106, provider profiles include **endpoint bindings**. What security problem do they solve? What happens if a profile lacks endpoint bindings?

---

### Q10 — Codex and `inference.local`

When running Codex inside an OpenShell sandbox with a BYO LLM, traffic goes through `inference.local`. Describe the role of `inference.local` (the privacy router), and explain why Codex needs the flag `--dangerously-bypass-approvals-and-sandbox` in this context.

---

### Q11 — Keycloak realm roles

The keycloak-oidc demo pre-configures four realm roles. List all four, explain who holds each, and describe how they map to OpenShell and MCP server access.

---

### Q12 — Admin onboarding workflow

Describe the complete sequence of admin steps to onboard a new user in the keycloak-oidc demo, starting from a deployed gateway+Keycloak. Include provider creation, refresh configuration, and what happens with the user's browser.

---

### Q13 — Claude Code vs. Codex API format

The demo supports both Codex and Claude Code with BYO LLM. What API format does each agent require, and why can't Claude Code use `inference.local` the same way Codex does?

---

### Q14 — `--secret-material-key` flag

When configuring a provider's refresh strategy, one value is marked with `--secret-material-key`. What does this flag do at the gateway level, and which specific credential value gets this treatment in the OIDC flow?

---

### Q15 — OpenShift SecurityContextConstraints

Which service account requires the `privileged` SCC for OpenShell sandboxes to work on OpenShift, and why is it specifically that account (not `default`)? Also, name the two `securityContext` fields that must be set to `null` in the Helm values for OpenShift compatibility.

---

### Q16 — MCP server pod architecture

Describe the two-container pod architecture of an MCP server in this demo. Specify which ports each container listens on, why the app container binds to loopback only, and what Envoy checks before forwarding a request.

---

### Q17 — Providers v2 collision protection

OpenShell's Providers v2 has a built-in safety mechanism when two providers are attached to the same sandbox. What does it check for, and what class of misconfiguration does it **not** catch?

---

### Q18 — User session workflow

Describe the complete sequence a user follows for a single working session — from first CLI command to cleanup — when OIDC is enabled. Include authentication, sandbox lifecycle, and what makes the credential injection transparent to the user.

---

### Q19 — gRPC transport on OpenShift

OpenShell's gateway communicates over gRPC. Explain why standard OpenShift edge or re-encrypt Routes break gRPC, and list the viable exposure options (at least two).

---

### Q20 — Onboard tool security model

The `onboard` CLI tool automates the OAuth flow for user onboarding. Explain why it uses the browser-based authorization code flow rather than a password grant, and what operational step the admin must take between onboarding two different users.
