# OpenShell + Keycloak OIDC + Agent Integration — Answer Key

---

### A1 — Supervisor credential handling

The gateway delivers real credential material to the **supervisor** ahead of time. The supervisor stores it locally inside the sandbox pod but **outside the sandbox process**. What the sandbox process sees in its environment variable (e.g. `$USER_ACCESS_TOKEN`) is a **resolve placeholder** — a string like `openshell:resolve:env:v168...` — not the actual secret.

When the sandbox process makes an outbound HTTP request using this placeholder in an `Authorization: Bearer` header, the supervisor's **policy proxy** intercepts the request, recognizes the resolve placeholder, swaps it for the real credential from its local store, and forwards the request upstream. The real token **never enters the sandbox process memory** — credential resolution is entirely local to the supervisor with no round-trip to the gateway at request time.

---

### A2 — mTLS + OIDC dual authentication

1. **Transport layer (mTLS):** The server verifies the client certificate, and the client verifies the server certificate. Client certs are extracted from the `openshell-client-tls` Kubernetes secret and stored at `~/.config/openshell/gateways/<name>/mtls/`.
2. **Application layer (JWT):** A JWT obtained from the OIDC login is sent in the gRPC `authorization` header on every request. The gateway validates the token's signature, issuer, audience, and expiry.

To activate OIDC, the Helm value `server.auth.allowUnauthenticatedUsers` must be changed from `true` (base install) to `false`. Additionally, `server.oidc.issuer`, `server.oidc.audience`, `server.oidc.rolesClaim`, `server.oidc.adminRole`, and `server.oidc.userRole` must be configured.

---

### A3 — Keycloak operator choice

The **Red Hat build of Keycloak** (`rhbk-operator`) from the **Red Hat Operators** catalog must be used. The project conventions make this explicit because searching for "keycloak" in the OperatorHub returns unrelated community operators that are not the correct choice.

Verify availability with:
```bash
oc get packagemanifests -n openshift-marketplace | grep rhbk-operator
```

---

### A4 — Provider profile vs. provider instance

- A **provider profile** is a template that defines *how* a credential type works: which environment variables it uses, the auth style (bearer, header), refresh strategy, endpoint bindings, and binary allowlists. Example: `user-scoped-api` — defines the `USER_ACCESS_TOKEN` env var with `oauth2_refresh_token` refresh strategy and MCP-server endpoint bindings.

- A **provider instance** is a concrete credential tied to a profile for a specific user or purpose. Example: `user-alice` — an instance of the `user-scoped-api` profile holding Alice's actual refresh token.

---

### A5 — Default sandbox network posture

By default, **all outbound network traffic is blocked**. Each sandbox gets its own network policy that denies egress.

An admin opens access using:
```bash
openshell policy update <sandbox> \
  --add-endpoint host:port:access:proto:enforce \
  --binary /path/to/binary
```

The two dimensions that must be specified together are:
1. **Endpoint** (host + port + access mode + protocol + enforcement)
2. **Binary** (the full path to the executable allowed to reach that endpoint)

---

### A6 — Envoy sidecar as the sole enforcement point

Defense-in-depth layers:
1. **Sandbox network policy** — only allowlisted endpoints are reachable at all.
2. **Envoy `jwt_authn` filter** — verifies the JWT signature against Keycloak's JWKS endpoint, validates the `iss` claim, and checks expiry.
3. **Envoy `rbac` filter** — inspects the `realm_access.roles` claim in the JWT and rejects requests that lack the server-specific role (e.g. `mcp-portfolio-user`).
4. **App loopback binding** — the MCP app listens on `127.0.0.1:8001`, making it unreachable from outside the pod; only Envoy (in the same pod) can reach it.

Removing Envoy is a critical failure because **the MCP server images themselves perform no authentication or authorization**. Testing confirmed that requests with **no token**, **garbage tokens**, and **valid tokens missing the required role** all successfully reached the tool endpoint when sent directly to the app container. Envoy is the **only** enforcement point.

---

### A7 — OIDC login trigger

In practice, `openshell gateway add` with the OIDC flags (`--oidc-issuer`, `--oidc-client-id`, `--oidc-scopes "openid offline_access"`) **already triggers the browser-based login flow** — the CLI opens the browser, the user authenticates with Keycloak, and the authorization code is exchanged for access + refresh tokens as part of the `gateway add` command.

The demo README includes a separate `openshell gateway login` step (step 2c) as an explicit confirmation, but it is redundant in practice because the login was already triggered by `gateway add`. A separate `gateway login` would only be needed if the session had expired or the user needed to re-authenticate.

---

### A8 — Resolve-placeholder lifecycle

1. **Delivery:** The gateway delivers the real credential material to the **supervisor** inside the sandbox pod ahead of time. The gateway's refresh worker continues to mint fresh short-lived access tokens using the stored refresh token and pushes updates to the supervisor.
2. **Env var:** The sandbox process's environment variable (e.g. `$USER_ACCESS_TOKEN`) contains a **resolve placeholder** (`openshell:resolve:env:v168...`), not the real token.
3. **Application use:** The application code uses the placeholder transparently — e.g. setting `Authorization: Bearer $USER_ACCESS_TOKEN` in an HTTP request. It does not need to know it's a placeholder.
4. **Supervisor interception:** The supervisor's **policy proxy** intercepts the outbound request, detects the resolve placeholder in the header, replaces it with the real credential from its local store, and forwards the request to the upstream API. The real secret never enters the sandbox process.

---

### A9 — Endpoint binding (0.0.106+)

Endpoint bindings solve the problem of **credential leakage to unintended destinations**. Without endpoint bindings, a credential attached to a sandbox could be resolved for any outbound request, regardless of where it was going.

With endpoint bindings in the provider profile, credentials are **only delivered to sandboxes when the profile includes matching endpoints** for the destinations the sandbox is configured to reach. If a profile lacks endpoint bindings, the credential may not be delivered to the supervisor, breaking credential injection.

---

### A10 — Codex and `inference.local`

`inference.local` is OpenShell's **privacy router** for LLM calls. It sits between the sandbox and the actual LLM provider. Its job: strip the caller's credentials at the proxy boundary and inject the real API key server-side. This means the sandbox never sees or handles the real LLM API key.

Codex requires `--dangerously-bypass-approvals-and-sandbox` because Codex's built-in sandbox uses **bubblewrap**, which attempts to create user namespaces. Inside an OpenShell sandbox container, user namespace creation is not possible. Since the OpenShell sandbox already provides external isolation, Codex's internal sandbox is redundant — the flag disables it so Codex can run at all.

---

### A11 — Keycloak realm roles

| Role | Assigned to | Purpose |
|---|---|---|
| `openshell-admin` | `openshell-admin` user | Full gateway admin operations — create sandboxes, manage providers, set policies |
| `openshell-user` | `alice`, `bob`, `charlie` | Connect to sandboxes, run workloads — the standard user role |
| `banker` | `alice`, `bob`, `charlie` | Composite role — grants the three roles below in one shot; baseline for any Meridian private banker |
| `mcp-portfolio-user` | Composited into `banker` | Access to `mcp-portfolio` (client holdings/performance) |
| `mcp-crm-calendar-user` | Composited into `banker` | Access to `mcp-crm-calendar` (banker's own meetings) |
| `mcp-market-news-user` | Composited into `banker` | Access to `mcp-market-news` (public market news) |
| `compatibility-user` | `alice` only, via the `compatibility-users` group | Access to `mcp-compatibility` (Compatibility Engine — `calc_tax` tool) — Alice's one extra permission, not shared with Bob or Charlie |

The `openshell-admin` and `openshell-user` roles are mapped via `server.oidc.adminRole` and `server.oidc.userRole` in Helm values. The MCP-server roles are enforced by the **Envoy `rbac` filter** on each MCP server pod. `banker` is a Keycloak *composite* role — Keycloak resolves its component roles into `realm_access.roles` automatically, so a caller just needs `banker` in their token; `compatibility-user` instead comes from **group role mapping** (membership in `compatibility-users`), not a direct role assignment on the user.

---

### A12 — Admin onboarding workflow

Starting from a deployed gateway + Keycloak with OIDC enabled:

1. **Import the provider profile** (if not already done):
   ```bash
   openshell provider profile import -f <profile-yaml>
   ```
   Registers the credential type: env vars, auth style, refresh strategy, and endpoint bindings.

2. **Create a provider instance** for the user (three-command pattern):
   ```bash
   # a) Create the provider with a pending credential
   openshell provider create \
     --name "user-<username>" --type user-scoped-api \
     --credential USER_ACCESS_TOKEN=pending
   ```

3. **Run the `onboard` tool** (or manually trigger the browser flow):
   - The admin runs the onboard tool with `--profile user-scoped-api`.
   - A browser opens to Keycloak's authorization endpoint.
   - The **user** (not the admin) authenticates with their own credentials — the admin never sees the password.
   - The tool captures the authorization code and exchanges it for a refresh token.

4. **Configure the refresh strategy** on the provider instance:
   ```bash
   # b) Bind the refresh strategy + per-user material
   openshell provider refresh configure "user-<username>" \
     --credential-key USER_ACCESS_TOKEN \
     --strategy oauth2-refresh-token \
     --secret-material-key refresh_token ...
   ```
   The refresh token is marked as secret material.

5. **Rotate to verify:**
   ```bash
   # c) Trigger first token exchange
   openshell provider refresh rotate "user-<username>"
   ```
   Triggers the first token exchange to confirm wiring works.

6. **Create a sandbox, attach the provider, and set policies** for the user.

**Important:** The admin must **log out of Keycloak** between onboarding different users, because browser session reuse would bind the wrong identity.

---

### A13 — Claude Code vs. Codex API format

- **Codex** requires the **OpenAI Responses API** format (`wire_api = "responses"`) with namespace tools. It uses `inference.local` as the privacy router, which speaks the OpenAI API format. It requires vLLM >= 0.25.0 (older versions reject `namespace` tools with HTTP 400).

- **Claude Code** requires the **Anthropic Messages API** format — it cannot use OpenAI-formatted endpoints. This means it **cannot** use `inference.local` (which is OpenAI-format only). Claude Code needs an Anthropic-compatible endpoint, such as DeepSeek's `/anthropic` endpoint or a LiteLLM proxy that translates to Anthropic format.

Non-secret config for Claude Code (base URL, model) is passed via `--env`, not provider `--config` — OpenShell only injects **credentials** as env vars.

---

### A14 — `--secret-material-key` flag

The `--secret-material-key` flag tells the gateway to treat the specified value as **sensitive material**. It is stored with additional protection and is never exposed in API responses, logs, or debug output.

In the OIDC flow, the **user's offline refresh token** receives this treatment. When running `openshell provider refresh configure`, the refresh token is passed with `--secret-material-key` so the gateway knows it's a secret that must be protected, while still using it internally to mint short-lived access tokens.

---

### A15 — OpenShift SecurityContextConstraints

The **`openshell-sandbox`** service account (not `default`) requires the `privileged` SCC. This is because sandbox pods run under the `openshell-sandbox` SA — it's the identity Kubernetes uses for the sandbox workload pods, which need elevated privileges for container isolation mechanisms.

The two `securityContext` fields that must be set to `null`:
1. `podSecurityContext.fsGroup: null`
2. `securityContext.runAsUser: null`

These must be null because OpenShift's admission controller needs to assign these values itself based on the namespace's SCC constraints. Hardcoding them in Helm values would conflict with OpenShift's security model.

---

### A16 — MCP server pod architecture

Each MCP server runs as a **two-container pod**:

1. **Envoy sidecar** — listens on **port 8000** (externally reachable via the Kubernetes Service). Performs JWT authentication (`jwt_authn` filter against Keycloak JWKS) and role-based authorization (`rbac` filter checking `realm_access.roles`). Only forwards requests that pass both checks to the app container.

2. **App container** — listens on **port 8001**, bound to **127.0.0.1 (loopback) only**. This means the app is unreachable from outside the pod — only Envoy, running in the same pod and sharing the same network namespace, can reach it.

Before forwarding, Envoy checks:
- JWT signature validity (against Keycloak's JWKS endpoint)
- Token issuer (`iss` claim)
- Token expiry
- Required role presence in `realm_access.roles`

---

### A17 — Providers v2 collision protection

Providers v2 **rejects** attaching two providers to the same sandbox if they expose the **same credential environment variable key** (e.g. both try to set `$USER_ACCESS_TOKEN`). This catches naming collisions.

However, it does **not** catch **misassignment** — OpenShell has no built-in concept of "this sandbox belongs to this user." If an admin attaches Alice's provider instance to Bob's sandbox, Providers v2 will not flag this. Getting the right provider attached to the right sandbox is entirely the **operator's responsibility**.

---

### A18 — User session workflow

1. **Connect to the gateway:**
   ```bash
   openshell gateway add <name> --address <host:port> \
     --oidc-issuer "https://<keycloak>/realms/openshell" \
     --oidc-client-id openshell-cli \
     --oidc-scopes "openid offline_access"
   ```
   This triggers the browser-based login — the user authenticates with Keycloak. Access + refresh tokens are obtained. The README includes a separate `openshell gateway login` step, but it is redundant in practice since `gateway add` already triggered the flow.

2. **Create or connect to a sandbox:**
   ```bash
   openshell sandbox create <name>    # or
   openshell sandbox connect <name>
   ```

3. **Work inside the sandbox:** Environment variables like `$USER_ACCESS_TOKEN` contain resolve placeholders. The user's application code uses them normally (e.g. in `Authorization: Bearer` headers). The **supervisor's policy proxy** transparently swaps placeholders for real credentials on outbound requests. The user never sees or handles real secrets.

4. **Cleanup:**
   ```bash
   openshell sandbox delete <name>
   ```

Credential injection is transparent because the resolve-placeholder mechanism requires **no application-side awareness** — standard `Authorization: Bearer $ENV_VAR` patterns work unchanged.

---

### A19 — gRPC transport on OpenShift

OpenShell's gateway communicates over **gRPC, which requires HTTP/2 end-to-end**. Standard OpenShift **edge** and **re-encrypt** Routes terminate TLS and re-establish the connection using HTTP/1.1, which breaks the HTTP/2 requirement and causes gRPC calls to fail.

Viable exposure options:
1. **`oc port-forward`** — tunnels raw TCP directly to the pod, preserving HTTP/2. Simple but not production-grade.
2. **Passthrough Route** — forwards raw TLS to the pod without terminating it, preserving HTTP/2. The Route hostname must be present in the server certificate's Subject Alternative Names (SANs).
3. **Envoy Gateway** (NVIDIA's recommended production path) — handles gRPC natively with full HTTP/2 support.

---

### A20 — Onboard tool security model

The `onboard` tool uses the **browser-based authorization code flow** rather than a password grant because:
- The operator **never sees the user's password**. The user authenticates directly with Keycloak in the browser.
- Password grant would require the admin to collect and type the user's credentials, creating a security risk (credential exposure to a third party).
- The authorization code flow is the standard OAuth 2.0 best practice for this reason.

Between onboarding two different users, the admin must **log out of the active Keycloak session** (in the browser). If they don't, the browser will reuse the previous user's session, causing the new onboarding flow to bind the wrong identity's refresh token to the new user's provider instance.
