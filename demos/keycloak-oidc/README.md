# Keycloak OIDC — per-user credential isolation on OpenShell

## Overview

This demo deploys OpenShell on OpenShift with **Keycloak as the OIDC identity
provider** and **per-user credential isolation** via Providers v2. Each user
gets their own scoped credential — an offline refresh token issued by
Keycloak — which the OpenShell gateway silently refreshes into short-lived
access tokens on that user's behalf. No shared service account, no
credential sharing between users.

The result is a multi-user setup where:

- An **admin** deploys the infrastructure (Keycloak, OpenShell, MCP servers)
  and onboards users.
- Each **user** connects to a sandbox that automatically carries their own
  identity. Outbound API calls from the sandbox use that user's own
  Keycloak-issued token — scoped, short-lived, and automatically rotated.
- Users are isolated from each other: user A's sandbox cannot access user
  B's credentials or reach services that user B is authorized for.

As a stretch goal, two example MCP servers are deployed behind Envoy
sidecars that enforce Keycloak role-based access — only users holding the
correct realm role can reach a given server.

### How to follow this guide

One person runs the entire demo, playing three roles: **admin** (steps 1-2),
**user1**, and **user2** (steps 3-4). A few practical tips:

- **Use separate terminal tabs** — one for admin work, one per user sandbox.
  The isolation check in step 4 requires user1's sandbox to stay running
  while you test user2, so you need both open at the same time.
- **Keycloak sessions are per-browser.** When onboarding multiple users with
  Option B (the `onboard` tool), log out of Keycloak between users —
  otherwise the browser reuses the previous session and you get the same
  user's token again. The tool's success page includes a logout link, or
  use a private/incognito window for each user.
- With Option A (password grant) there is no browser session to worry about
  — just change `USER_ID` and `USER_PASS` in the same terminal.

## RBAC setup

This demo uses four Keycloak realm roles to separate admin and user
capabilities:

| Role | Who holds it | What it grants |
|---|---|---|
| `openshell-admin` | The demo admin | Full OpenShell gateway admin operations (deploy providers, manage sandboxes, set policies) |
| `openshell-user` | Every onboarded user | Connect to sandboxes, run workloads |
| `mcp-server-a-user` | Users authorized for MCP server A | Access to the Eligibility Engine MCP server |
| `mcp-server-b-user` | Users authorized for MCP server B | Access to the Compatibility Engine MCP server |

```mermaid
flowchart TB
    subgraph Admin["Admin flow (once)"]
        A1[Deploy Keycloak] --> A2[Apply OIDC overlay]
        A2 --> A3[Enable Providers v2]
        A3 --> A4[Deploy MCP servers]
    end

    subgraph Onboard["Per-user onboarding"]
        O1[User logs in via OAuth] --> O2[Obtain offline refresh token]
        O2 --> O3[Store token in OpenShell provider]
    end

    subgraph Use["Per-user usage"]
        U1[Create sandbox] --> U2[Attach provider + policy]
        U2 --> U3[Sandbox carries user's own token]
        U3 --> U4[API calls scoped to user's identity]
    end

    A4 --> Onboard
    Onboard --> Use
```

## Prerequisites

| Tool / access | Notes |
|---|---|
| `oc` | Logged into the target cluster, with rights to grant SCCs |
| `helm` 3.x | |
| `kubectl` | Compatible with cluster version |
| `openshell` CLI | See [`demos/base/README.md`](../base/README.md#installing-the-cli) for install instructions |
| OpenShift 4.x cluster | |
| Agent Sandbox controller + CRDs | See [`demos/base/README.md`](../base/README.md#installing-agent-sandbox) |
| A Keycloak instance (26+, or current) | Self-hosted via Helm, or existing |
| `jq`, `openssl` | Scripting, secret handling |

## What this demo deploys

- `helm/values.yaml` — OpenShift-compatibility overrides plus OIDC
  configuration (`server.oidc.*`, `allowUnauthenticatedUsers: false`).
- A Keycloak realm (`keycloak/realm-export.json`) with CLI and
  gateway clients, admin/user roles, and a few demo users. The gateway
  client secret is hardcoded (`openshell-gateway-demo-secret`) — in a
  production setup you would generate a unique secret per environment.
- Providers v2 enabled (`providers_v2_enabled=true`).
- A per-user provider profile and onboarding flow.
- Two example MCP servers (`mcp-servers/` chart) fronted by Envoy sidecars
  that gate access by Keycloak realm role.

## Architecture

```mermaid
flowchart TB
    subgraph User["Per-user, repeats for each tenant"]
        C[User logs in] --> B[Backend stores<br/>user's refresh token]
    end
    B --> P["Provider instance<br/>user-&lt;id&gt;"]
    P --> SB[Sandbox created<br/>with --provider attached]
    SB --> GW[OpenShell Gateway]
    GW <--> KC[(Keycloak<br/>realm: openshell)]
    GW --> API[Downstream API<br/>call, user-scoped token]

    subgraph OCP["OpenShift cluster"]
        GW
        SB
    end
```

## Getting started

### Clone the repo and change to the demo directory

```bash
git clone https://github.com/alpha-hack-program/openshell-demos.git
cd openshell-demos/demos/keycloak-oidc
```

### Log into your OpenShift cluster

Make sure you're logged in with a user that has **cluster-admin** rights (or
at least the ability to grant SCCs and create namespaces):

```bash
oc login --server=https://api.<your-cluster>:6443
oc whoami   # confirm you're logged in
```

### Set up your `.env` files

This demo uses two `.env` files:

1. **Root `.env`** (at the repo root) — cluster-wide variables shared across
   all demos:
   - `OPENSHELL_CHART_VERSION` — the Helm chart version to install
   - `CLUSTER_APPS_DOMAIN` — your cluster's apps domain

2. **Demo `.env`** (in this directory) — variables specific to this demo.

Copy the example files and fill in the real values:

```bash
# Root .env — cluster-wide variables
cp ../../.env.example ../../.env
```

Extract `CLUSTER_APPS_DOMAIN` from the cluster:

```bash
CLUSTER_APPS_DOMAIN=$(oc get ingresses.config.openshift.io cluster -o jsonpath='{.spec.domain}')
echo "CLUSTER_APPS_DOMAIN=${CLUSTER_APPS_DOMAIN}"
```

To find the latest chart version, check the
[OpenShell releases page](https://github.com/NVIDIA/OpenShell/releases) — the
tag uses a `v` prefix (e.g. `v0.0.106`) but the chart version does **not**
(e.g. `0.0.106`). You can also query it directly:

```bash
helm show chart oci://ghcr.io/nvidia/openshell/helm-chart | grep ^version
```

Edit `../../.env` and set `OPENSHELL_CHART_VERSION` (without the `v` prefix)
and paste the `CLUSTER_APPS_DOMAIN` value from above.

```bash
# Demo .env — demo-specific variables
cp .env.example .env
# Edit .env — values are listed below
```

The demo `.env` requires these variables. Don't worry about filling them all
in now — the `01-deploy-keycloak.sh` script in step 1 prints the
Keycloak-related values (`KEYCLOAK_HOST`, `KEYCLOAK_CLIENT_SECRET`, etc.)
after it runs, so you'll come back and complete your `.env` then.

| Variable | Example | Notes |
|---|---|---|
| `OPENSHELL_NAMESPACE` | `keycloak-oidc-demo` | OpenShift namespace for this demo's gateway. Must **not** start with `openshell-` — the Route FQDN is derived as `openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}`, and a redundant prefix can push it over the 64-byte X.509 CommonName limit when using `LETSENCRYPT_CLUSTER_ISSUER` (see AGENTS.md) |
| `CERT_MANAGER` | `false` | Set `true` to use cert-manager for TLS (requires the Operator) |
| `LETSENCRYPT_CLUSTER_ISSUER` | *(empty)* | Name of a Let's Encrypt `ClusterIssuer` for CA-signed Route cert (requires `CERT_MANAGER=true`) |
| `KEYCLOAK_HOST` | `keycloak.apps.ocp.example.com` | Keycloak hostname (set after deploying Keycloak) |
| `KEYCLOAK_REALM` | `openshell` | Keycloak realm name |
| `KEYCLOAK_CLIENT_ID_CLI` | `openshell-cli` | Public client for CLI/browser login |
| `KEYCLOAK_CLIENT_ID_GATEWAY` | `openshell-gateway` | Confidential gateway client |
| `KEYCLOAK_CLIENT_SECRET` | *(from Keycloak)* | Gateway client secret — never commit |

## Steps

### 1. Deploy Keycloak

#### 1a. Check the realm JSON

The realm JSON at `keycloak/realm-export.json` is ready to import as-is —
no substitution needed. The gateway client secret is hardcoded as
`openshell-gateway-demo-secret` (along with demo user passwords
`user1`/`user1`, `user2`/`user2`). This keeps the demo simple; in
production you would generate a unique secret per environment.

Optionally, run the helper script to verify your `.env` values match:

```bash
./scripts/01-deploy-keycloak.sh
```

#### 1b. Deploy Keycloak on the cluster

We use the **Red Hat build of Keycloak** (RHBK) Operator, available from
OperatorHub on OpenShift. The package name is **`rhbk-operator`** (in the
**Red Hat Operators** catalog) — do **not** use the community
`keycloak-operator` from Community Operators.

1. Create the namespace first, then install the Operator into it:

   ```bash
   oc create namespace keycloak 2>/dev/null || true
   ```

   **Via the web console:** go to **Operators > OperatorHub**, search for
   **`rhbk`**, and install the **Red Hat build of Keycloak** Operator.
   Select **A specific namespace on the cluster** and choose `keycloak`.

   **Via the CLI:**

   ```bash
   # Verify the package is available
   oc get packagemanifests -n openshift-marketplace | grep rhbk-operator

   # Create OperatorGroup + Subscription
   oc apply -f - <<'EOF'
   apiVersion: operators.coreos.com/v1
   kind: OperatorGroup
   metadata:
     name: keycloak-og
     namespace: keycloak
   spec:
     targetNamespaces:
       - keycloak
   ---
   apiVersion: operators.coreos.com/v1alpha1
   kind: Subscription
   metadata:
     name: rhbk-operator
     namespace: keycloak
   spec:
     channel: stable-v26.6
     name: rhbk-operator
     source: redhat-operators
     sourceNamespace: openshift-marketplace
     installPlanApproval: Automatic
   EOF
   ```

2. Wait for the Operator to be ready:

   ```bash
   oc -n keycloak get csv | grep rhbk
   # Should show a row with "Succeeded" for the rhbk-operator
   ```

3. Source your root `.env` (for `CLUSTER_APPS_DOMAIN`) and create a Keycloak
   instance. Note the heredoc uses `<<EOF` (no quotes) so the variable is
   expanded:

   ```bash
   source ../../.env

   oc -n keycloak apply -f - <<EOF
   apiVersion: k8s.keycloak.org/v2beta1
   kind: Keycloak
   metadata:
     name: keycloak
   spec:
     instances: 1
     hostname:
       hostname: keycloak.${CLUSTER_APPS_DOMAIN}
     proxy:
       headers: xforwarded
     http:
       httpEnabled: true
   EOF
   ```

   The `proxy.headers` and `http.httpEnabled` settings are needed because the
   OpenShift Route handles TLS termination — Keycloak itself runs behind the
   Route over plain HTTP.

   > If `keycloak.${CLUSTER_APPS_DOMAIN}` is already taken on your cluster,
   > change the hostname in the CR above (e.g.
   > `keycloak-openshell.${CLUSTER_APPS_DOMAIN}`) and update `KEYCLOAK_HOST`
   > in your `.env` to match.

4. Wait for the pod to become ready:

   ```bash
   oc -n keycloak get pods
   # keycloak-0   1/1   Running
   ```

5. Grab the admin credentials created by the Operator — you'll need them
   to log into the admin console in the next step:

   ```bash
   oc -n keycloak get secret keycloak-initial-admin \
     -o jsonpath='{.data.username}' | base64 -d; echo
   oc -n keycloak get secret keycloak-initial-admin \
     -o jsonpath='{.data.password}' | base64 -d; echo
   ```

#### 1c. Import the realm JSON

Import the realm JSON into Keycloak via the admin console or the Admin
REST API.

**Option A — Admin console (browser):**

1. Open `https://<KEYCLOAK_HOST>/admin` in your browser (use the admin
   credentials you extracted in step 1b).
2. In the left sidebar, click **Manage realms**, then click **Create realm**.
3. Click **Browse**, select `keycloak/realm-export.json`, and click
   **Create**.

**Option B — Admin REST API (CLI):**

```bash
source .env

ADMIN_TOKEN=$(curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/realms/master/protocol/openid-connect/token" \
  -d "grant_type=password" \
  -d "client_id=admin-cli" \
  -d "username=${KEYCLOAK_ADMIN_USER}" \
  -d "password=${KEYCLOAK_ADMIN_PASSWORD}" \
  | jq -r '.access_token')

curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/admin/realms" \
  -H "Authorization: Bearer ${ADMIN_TOKEN}" \
  -H "Content-Type: application/json" \
  -d @keycloak/realm-export.json
```

The realm template includes two demo users (`user1` / `user2`) with the
`openshell-user` role and `offline_access` scope — they're created
automatically when you import the realm. Their passwords match their
usernames (e.g. `user1` / `user1`) — demo only, never do this in production.

### 2. Create the namespace, grant SCCs, and install OpenShell with OIDC

This step installs the OpenShell gateway with OIDC and a passthrough Route
in a single Helm install (2a), then extracts the mTLS client certificates
and registers the gateway endpoint with the `openshell` CLI (2b).

> **If you already have another OpenShell installation** on the same cluster
> (the base demo, a previous run of this demo in a different namespace, etc.),
> the Helm install below will fail because the chart creates cluster-scoped
> resources (`openshell-node-reader` ClusterRole **and** ClusterRoleBinding)
> that are already owned by the other release. **Tear down the previous
> installation first** — e.g. `demos/base/scripts/99-teardown.sh` for the
> base demo, or `demos/keycloak-oidc/scripts/99-teardown.sh` for a prior run
> of this demo.

#### 2a. Helm install

This demo provides two Helm values files:

- **`helm/values.yaml`** — uses the PKI init job to generate TLS certificates
  (default, no extra dependencies).
- **`helm/values-certmanager.yaml`** — uses the OpenShift cert-manager
  Operator to manage TLS certificates. Requires the cert-manager Operator to
  be installed on the cluster. Set `CERT_MANAGER=true` in your `.env` to use
  this path.

Both files contain a `<keycloak-host>` placeholder in the OIDC issuer URL
and `openshiftRoute.enabled: true`. Use `--set` to fill in your Keycloak
hostname and the Route FQDN at install time. The chart creates the
passthrough Route automatically — no manual `oc create route` needed.

The Route hostname must also be in the server certificate's SANs — without
it, TLS handshakes will fail. The `--set` overrides below add it alongside
the namespace-specific service DNS names.

##### Let's Encrypt TLS (optional)

When using the cert-manager path (`CERT_MANAGER=true`), you can get a
publicly-trusted CA-signed certificate for the Route instead of the default
self-signed CA. The chart's `serverIssuerRef` creates a **second** server
certificate signed by your ACME issuer (for the Route FQDN) while the
**internal** certificate stays signed by the chart's own CA — mTLS client
auth keeps working unchanged. The gateway serves the right certificate via
SNI.

To use this:

1. Set `LETSENCRYPT_CLUSTER_ISSUER` in your `.env` to the name of an
   existing Let's Encrypt `ClusterIssuer` on your cluster (e.g.
   `letsencrypt-prod`).

2. If you don't have a ClusterIssuer yet, create one. DNS-01 is recommended
   for passthrough Routes (HTTP-01 needs port 80, which passthrough doesn't
   expose). The solver configuration depends on your DNS provider — this
   example uses Route53, but cert-manager supports
   [many providers](https://cert-manager.io/docs/configuration/acme/dns01/):

   ```bash
   oc apply -f - <<'EOF'
   apiVersion: cert-manager.io/v1
   kind: ClusterIssuer
   metadata:
     name: letsencrypt-prod
   spec:
     acme:
       server: https://acme-v02.api.letsencrypt.org/directory
       email: your-email@example.com        # Let's Encrypt notifications
       privateKeySecretRef:
         name: letsencrypt-prod-account-key
       solvers:
         - dns01:
             route53:                        # replace with your DNS provider
               region: us-east-1
               # accessKeyID / secretAccessKeySecretRef or IRSA — see
               # cert-manager docs for your provider
   EOF

   # Verify the issuer is ready
   oc get clusterissuer letsencrypt-prod
   ```

The helm install snippet below automatically passes `serverIssuerRef` when
`LETSENCRYPT_CLUSTER_ISSUER` is set. If left empty, the chart uses its own
self-signed CA for all certificates (the default).

When `serverIssuerRef` is set, `certManager.serverDnsNames` feeds **only**
the external (ACME) certificate — the internal certificate's SANs come
from the chart's own defaults automatically, regardless of this value. The
external certificate's `dnsNames` are used as-is, with no filtering, so
the list must contain **only** the externally-resolvable Route FQDN — no
internal names (rejected by the chart's own guard), no IPs, and no bare
single-label names (both silently accepted by the chart but rejected by
ACME). That's why the two branches below differ: the self-signed-CA branch
appends the Route host to the base internal-SAN list, while the Let's
Encrypt branch replaces `serverDnsNames` wholesale with just the Route
host. Also see the `OPENSHELL_NAMESPACE` naming constraint above — the
Route host doubles the `openshell-` prefix if the namespace already starts
with it, which can push the Let's Encrypt certificate's CommonName over
the 64-byte X.509 limit.

```bash
source .env
source ../../.env

oc create namespace "$OPENSHELL_NAMESPACE" 2>/dev/null || true
oc adm policy add-scc-to-user privileged -z openshell-sandbox -n "$OPENSHELL_NAMESPACE"

ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"

if [[ "${CERT_MANAGER:-false}" == "true" ]]; then
  VALUES_FILE=helm/values-certmanager.yaml
  ISSUER_SET=()
  if [[ -n "${LETSENCRYPT_CLUSTER_ISSUER:-}" ]]; then
    # serverDnsNames feeds ONLY the external (ACME) certificate when
    # serverIssuerRef is set — replace it wholesale with just the
    # externally-resolvable Route host (no internal names, no IPs, no
    # bare names; ACME rejects all three).
    SAN_SET=(--set "certManager.serverDnsNames={${ROUTE_HOST}}")
    ISSUER_SET=(
      --set "certManager.serverIssuerRef.name=${LETSENCRYPT_CLUSTER_ISSUER}"
      --set "certManager.serverIssuerRef.kind=ClusterIssuer"
      --set "certManager.serverIssuerRef.group=cert-manager.io"
    )
  else
    SAN_SET=(
      --set "certManager.serverDnsNames[2]=openshell.${OPENSHELL_NAMESPACE}.svc"
      --set "certManager.serverDnsNames[3]=openshell.${OPENSHELL_NAMESPACE}.svc.cluster.local"
      --set "certManager.serverDnsNames[4]=${ROUTE_HOST}"
    )
  fi
else
  VALUES_FILE=helm/values.yaml
  SAN_SET=(--set "pkiInitJob.serverDnsNames[0]=${ROUTE_HOST}")
  ISSUER_SET=()
fi

helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" \
  --namespace "$OPENSHELL_NAMESPACE" \
  -f "$VALUES_FILE" \
  --set "server.oidc.issuer=https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}" \
  --set "openshiftRoute.host=${ROUTE_HOST}" \
  "${SAN_SET[@]}" \
  "${ISSUER_SET[@]}"
```

Wait for the gateway to come up:

```bash
oc -n "$OPENSHELL_NAMESPACE" rollout status statefulset/openshell
```

Verify the Route was created:

```bash
oc -n "$OPENSHELL_NAMESPACE" get route openshell
```

#### 2b. Register the gateway with the CLI

Extract the client mTLS certificates and register the gateway:

```bash
GATEWAY_NAME="${GATEWAY_NAME:-openshift}"
MTLS_DIR=~/.config/openshell/gateways/${GATEWAY_NAME}/mtls
mkdir -p "$MTLS_DIR"

oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"

# If using the Let's Encrypt path (LETSENCRYPT_CLUSTER_ISSUER set), the CLI's
# gRPC control channel pins trust to this ca.crt bundle and does NOT fall back
# to the system trust store. Since $ROUTE_HOST now serves a Let's
# Encrypt-signed certificate (via SNI) instead of the chart's own CA, append
# its issuing chain — otherwise every CLI command fails with
# "invalid peer certificate: UnknownIssuer".
if [[ -n "${LETSENCRYPT_CLUSTER_ISSUER:-}" ]]; then
  echo | openssl s_client -connect "${ROUTE_HOST}:443" -servername "${ROUTE_HOST}" -showcerts 2>/dev/null \
    | awk '/-----BEGIN CERTIFICATE-----/{n++} n>=2' >> "$MTLS_DIR/ca.crt"
fi

openshell gateway remove "$GATEWAY_NAME" 2>/dev/null || true
openshell gateway add "https://${ROUTE_HOST}:443" \
  --name "$GATEWAY_NAME" \
  --oidc-issuer "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}" \
  --oidc-client-id "$KEYCLOAK_CLIENT_ID_CLI" \
  --oidc-scopes "openid offline_access"
```

#### 2c. Log in to the gateway

The gateway requires OIDC authentication — you must log in before you can
run any admin commands. This opens a browser and redirects you to Keycloak:

```bash
openshell gateway login
```

Log in as the admin user (the one with the `openshell-admin` realm role).

#### 2d. Enable Providers v2

```bash
openshell settings set --global --key providers_v2_enabled --value true
```

#### Verify the gateway

```bash
openshell status
openshell gateway list
```

`openshell status` should show the gateway as connected and authenticated.

### 3. Onboard a user

User onboarding is a **two-step process by design**. OpenShell's Providers v2
manages the *lifecycle* of a credential (refresh, rotate, inject into
sandboxes) but leaves the *initial acquisition* of that credential to your
identity plumbing. The upstream docs jump straight to
`--material refresh_token=<value>` and assume you already have it — this
section explains how to get it.

You need a long-lived **offline refresh token** (not a short-lived access
token) because the gateway uses it to silently mint fresh access tokens on
the user's behalf over time, without the user being logged in.

Two options for obtaining the token:

- **Option A** uses a direct password grant — fast but demo-only, since the
  operator must know the user's password.
- **Option B** uses the `onboard` CLI tool to automate a browser-based
  OAuth flow — the user logs in directly with Keycloak and the operator
  never sees their password.

#### Step 3a — Obtain the user's refresh token

**Option A — Password grant (demo only)**

Only works because you control both sides and know the demo user's password.
Not viable in production — the operator must never know user credentials.

```bash
source .env

USER_ID="user1"
USER_PASS="<the-users-password>"

REFRESH_TOKEN=$(curl -sk -X POST \
  "https://${KEYCLOAK_HOST}/realms/${KEYCLOAK_REALM}/protocol/openid-connect/token" \
  -d "grant_type=password" \
  -d "client_id=${KEYCLOAK_CLIENT_ID_CLI}" \
  -d "username=${USER_ID}" \
  -d "password=${USER_PASS}" \
  -d "scope=openid offline_access" \
  | jq -r '.refresh_token')
```

Then continue to [step 3b](#step-3b--store-the-refresh-token-in-openshell) to
store the token.

**Option B — `onboard` utility (browser-based OAuth flow)**

The `onboard` CLI tool in [`util/onboard/`](../../util/onboard/) automates
the full flow: opens the browser for the user to log in, listens for the
OAuth callback, exchanges the authorization code for a refresh token, and
runs the OpenShell provider commands from
[step 3b](#step-3b--store-the-refresh-token-in-openshell) automatically.

```bash
source .env

# Build once (requires Rust)
cd ../../util/onboard && cargo build --release && cd -

# Onboard user1 — opens a browser, waits for login, creates the provider
../../util/onboard/target/release/onboard \
  -u user1 \
  --profile providers/user-refresh-profile.yaml
```

Or use the shell wrapper (sources `.env` automatically):

```bash
../../util/onboard/onboard.sh \
  -u user1 \
  --profile providers/user-refresh-profile.yaml
```

The `--profile` flag is required — it tells the tool which provider profile
to import. The profile defines the credential refresh strategy (how the
gateway obtains fresh access tokens from Keycloak on the user's behalf). See
[step 3b](#step-3b--store-the-refresh-token-in-openshell) for what the
profile contains and why it matters.

Pre-built binaries for Linux (x86_64) and macOS (aarch64) are available from
[GitHub Releases](../../releases) — download, `chmod +x`, and run.

Useful flags:
- `--token-only` — stop after obtaining the refresh token, print it to
  stdout, do not call the OpenShell CLI
- `--no-browser` — print the URL instead of opening a browser (for
  headless / SSH sessions)
- `--dry-run` — show the OpenShell CLI commands without executing them
- `--timeout <secs>` — how long to wait for the user to log in (default 120s)

To onboard a second user, log out of Keycloak first (use the link on the
success page or open an incognito window), then run the same command with
`-u user2`.

If you used Option B, step 3b is already done — the tool runs the same
commands shown below. Skip to [step 4](#4-deploy-mcp-servers).

#### Step 3b — Store the refresh token in OpenShell

This is what OpenShell owns. The **provider profile** defines the credential
refresh strategy — how the gateway obtains fresh access tokens from Keycloak
on the user's behalf. The **provider instance** stores each user's refresh
token and is linked to the profile.

The profile defines both the credential refresh strategy and the
**endpoint binding** — which endpoints the proxy is allowed to inject the
credential for. In 0.0.106+, credentials are only delivered to sandboxes
when the profile includes matching endpoints (this is a security
enhancement that prevents credentials from leaking to unintended
destinations). Envoy RBAC at each MCP server still enforces role-based
access — the endpoint binding only tells the proxy "you may inject this
credential for requests to these hosts."

First, import the provider profile. The profile at
`providers/user-refresh-profile.yaml` contains two placeholders:
`<keycloak-host>` in `token_url` and `<openshell-namespace>` in the MCP
server endpoint hostnames. Replace both with your actual values:

```bash
source .env

TMPFILE=$(mktemp --suffix=.yaml)
sed -e "s|<keycloak-host>|${KEYCLOAK_HOST}|" \
    -e "s|<openshell-namespace>|${OPENSHELL_NAMESPACE}|" \
    providers/user-refresh-profile.yaml > "$TMPFILE"
openshell provider profile import -f "$TMPFILE"
rm -f "$TMPFILE"
```

Then create a provider for the user and configure automatic token refresh:

```bash
USER_ID="user1"

# Create the provider — this links the user to the profile's refresh strategy
openshell provider create \
  --name "user-${USER_ID}" \
  --type user-scoped-api \
  --credential USER_ACCESS_TOKEN=pending

# Store the user's refresh token and configure automatic rotation
openshell provider refresh configure "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN \
  --strategy oauth2-refresh-token \
  --material client_id="${KEYCLOAK_CLIENT_ID_CLI}" \
  --material refresh_token="${REFRESH_TOKEN}" \
  --secret-material-key refresh_token

# Trigger the first rotation to verify everything works
openshell provider refresh rotate "user-${USER_ID}" \
  --credential-key USER_ACCESS_TOKEN
```

From this point forward, the gateway's refresh worker automatically mints
short-lived access tokens and rotates the refresh token whenever the IdP
returns a new one. The user just does `openshell sandbox connect` and gets a
sandbox with a live credential — they never see a token.

> **Why three separate commands?** A provider profile can define *multiple*
> credentials, each with its own refresh strategy, token endpoint, and
> timing. `provider create` registers the credential keys the provider will
> manage. `refresh configure` binds a specific strategy and the per-user
> material (the actual refresh token) to each credential key — one call per
> key. `refresh rotate` triggers the first token exchange to verify the
> wiring. The `--strategy` flag on `refresh configure` is not redundant
> with the profile: when a profile declares several credentials with
> different strategies, the flag tells the gateway which strategy applies to
> which credential key on this particular provider instance.

> The script `scripts/03-onboard-user.sh <user-id> <refresh-token>` wraps
> these same commands.

### 4. Deploy MCP servers

Two example downstream services (MCP servers) that validate the caller's
Bearer token as a Keycloak-issued OAuth access token — the same token
Providers v2 already mints/refreshes per user in step 3 — and are only
reachable by users holding a specific Keycloak realm role.

Token enforcement is handled by an **Envoy sidecar** in front of each MCP
server. Envoy's `jwt_authn` filter verifies the token's signature against
Keycloak's JWKS and `iss`; its `rbac` filter requires the decoded
`realm_access.roles` claim to contain the server-specific role. The app
itself listens on loopback only (`127.0.0.1:8001`) and is unreachable except
from Envoy in the same pod.

Each server has its own Keycloak realm role (`mcp-server-a-user`,
`mcp-server-b-user`).

```bash
source .env
./scripts/06-deploy-mcp-servers.sh
```

This deploys `mcp-server-a` and `mcp-server-b` into `$OPENSHELL_NAMESPACE`
as two-container pods (Envoy + the app), each with its own ServiceAccount.

The MCP server roles are already assigned to the demo users in the realm
JSON imported in step 1c (`mcp-server-a-user` → `user1`,
`mcp-server-b-user` → `user2`). To onboard additional users beyond the
two pre-configured ones, see
[Onboarding additional users](docs/onboard-additional-users.md).

### 5. Run the demo

Create a sandbox with the user's provider, add binary permissions, then
verify that MCP calls succeed with the user's own scoped token — and that
cross-user isolation holds.

```bash
source .env
USER_ID="user1"
SERVER_NAME="mcp-server-a"
MCP_URL="http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"
```

**Create the sandbox with the provider attached.** The `--provider` flag
attaches the user's credential at creation time — this injects
`$USER_ACCESS_TOKEN` as a resolve placeholder that the supervisor's proxy
resolves to a real Keycloak access token on matching outbound requests.
The `-- true` creates the sandbox without entering an interactive shell:

```bash
openshell sandbox create --name "demo-${USER_ID}" \
  --provider "user-${USER_ID}" \
  -- true
```

**Add binary permissions** so curl can reach the MCP server. The provider
profile already contributes the MCP server endpoint to the sandbox's
network policy (endpoint binding), but does not grant binary-level
permissions — those are deployment-specific and applied per-sandbox:

```bash
openshell policy update "demo-${USER_ID}" \
  --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
  --binary /usr/bin/curl --wait
```

**Verify** — the same request with the user's token should now succeed.
`$USER_ACCESS_TOKEN` is injected into the sandbox by the provider:

```bash
openshell sandbox exec -n "demo-${USER_ID}" --env "MCP_URL=${MCP_URL}" \
  -- bash -c 'curl -sS \
    -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$MCP_URL"'
# Expected: 200 — MCP server returns its capabilities
```

**Isolation check** — user1 should not be able to reach `mcp-server-b`
(different realm role required). Note: use `--env` to pass host-side
variables into the sandbox — they are not available inside single quotes:

```bash
OTHER_MCP_URL="http://mcp-server-b.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

openshell sandbox exec -n "demo-${USER_ID}" --env "OTHER_MCP_URL=${OTHER_MCP_URL}" \
  -- bash -c 'curl -so /dev/null -w "%{http_code}" \
    -X POST \
    -H "Authorization: Bearer $USER_ACCESS_TOKEN" \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":\"2025-03-26\",\"capabilities\":{},\"clientInfo\":{\"name\":\"test\",\"version\":\"0.1\"}}}" \
    "$OTHER_MCP_URL"'
# Expected: 403 — valid token, but user1 lacks the mcp-server-b-user role
```

To test the other direction, open a second terminal and repeat with
`USER_ID="user2"` and `SERVER_NAME="mcp-server-b"` (onboard user2 first
if you haven't already). Confirm user2 can reach `mcp-server-b` (`200`)
but gets `403` from `mcp-server-a`.

#### Test recipe: Codex + BYO LLM + MCP tool

Codex CLI calling an MCP server's tool via your own OpenAI-compatible LLM.
Codex uses `inference.local` — OpenShell's privacy router — which strips
caller credentials at the proxy boundary and injects the real API key
server-side.

> **LLM endpoint requirements.** Codex 0.146.0+ only supports
> `wire_api = "responses"` (the OpenAI Responses API). It sends MCP tools
> as `"type": "namespace"` tools — a Responses API extension that groups
> MCP tools under a named scope. This means your LLM endpoint must support
> the Responses API **with namespace tools**, which requires **vLLM >=
> 0.25.0** (or OpenAI's own API). Older vLLM versions accept the Responses
> API but reject `namespace` tools with a 400 error. See
> [`docs/inference-api-compatibility.md`](docs/inference-api-compatibility.md)
> for the full compatibility matrix and a test script.

**Prerequisites** beyond steps 1-5 above — set `OPENAI_API_KEY`,
`OPENAI_BASE_URL`, and `OPENAI_MODEL` in your `.env` (see `.env.example`),
then in your **admin terminal**:

```bash
source .env
USER_ID="user1"
SERVER_NAME="mcp-server-a"
QUESTION="My mother is at the hospital, can I get an aid while I am on unpaid leave?"
```

1. Create the inference provider and configure `inference.local` routing
   (only type `openai` providers can drive `inference.local`):

   ```bash
   openshell provider create --name byo-inference --type openai \
     --credential "OPENAI_API_KEY=$OPENAI_API_KEY" \
     --config "OPENAI_BASE_URL=$OPENAI_BASE_URL"

   openshell inference set \
     --provider byo-inference \
     --model "$OPENAI_MODEL" \
     --timeout 120
   ```

2. Import the Codex policy profile and create a second provider for binary
   permissions.

   The profile ([`providers/byo-codex-profile.yaml`](providers/byo-codex-profile.yaml))
   defines which binaries Codex needs (`codex`, its Node modules), locks
   network access to `inference.local:443` (the OpenShell privacy router),
   and injects the API key as `OPENAI_API_KEY`:

   ```yaml
   id: byo-codex
   display_name: BYO LLM (Codex policy)
   description: Network policy and binary permissions for Codex via inference.local
   category: inference
   inference_capable: true

   credentials:
     - name: api_key
       description: LLM API key (injected as OPENAI_API_KEY for Codex)
       env_vars: [OPENAI_API_KEY]
       required: true
       auth_style: bearer
       header_name: authorization

   endpoints:
     - host: inference.local
       port: 443
       protocol: rest
       access: read-write
       enforcement: enforce

   binaries:
     - /usr/bin/codex
     - /usr/local/bin/codex
   ```

   Import it and create the provider:

   ```bash
   openshell provider profile import -f providers/byo-codex-profile.yaml
   openshell provider create --name byo-codex --type byo-codex \
     --credential "OPENAI_API_KEY=$OPENAI_API_KEY"
   ```

3. Create and configure the sandbox. Use a custom image with Codex >=
   0.146.0 if the chart's default sandbox image ships an older version.

   Generate the Codex config locally (model provider + MCP server
   registration), then inject it at sandbox creation time with `--upload`
   so the sandbox starts ready — no `sandbox exec` needed:

   ```bash
   CODEX_IMAGE="quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012"  # Codex 0.146.0
   MCP_URL="http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp"

   CODEX_CONFIG=$(mktemp)
   cat > "$CODEX_CONFIG" <<EOF
   model_provider = "openshell-byo"
   model = "${OPENAI_MODEL}"

   [model_providers.openshell-byo]
   name = "OpenShell BYO Router"
   base_url = "https://inference.local/v1"
   env_key = "OPENAI_API_KEY"
   wire_api = "responses"

   [mcp_servers.${SERVER_NAME}]
   url = "${MCP_URL}"
   bearer_token_env_var = "USER_ACCESS_TOKEN"

   [projects."/sandbox"]
   trust_level = "trusted"
   EOF

   openshell sandbox create --name "codex-${USER_ID}" \
     --provider byo-codex \
     --provider "user-${USER_ID}" \
     --from "${CODEX_IMAGE}" \
     --upload "${CODEX_CONFIG}:/sandbox/.codex/config.toml" \
     -- true

   rm -f "$CODEX_CONFIG"

   openshell policy update "codex-${USER_ID}" \
     --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
     --binary /usr/local/bin/codex \
     --wait
   ```

   > The `--upload` flag takes `<LOCAL_PATH>:<SANDBOX_PATH>` — specify the
   > full file path on both sides (uploading a directory nests it as a
   > subdirectory inside the target). The sandbox home is `/sandbox`, so
   > Codex's config directory is `/sandbox/.codex/`.

4. Run the test from the **user terminal**:

   ```bash
   source .env
   USER_ID="user1"
   QUESTION="My mother is at the hospital, can I get an aid while I am on unpaid leave?"

   # The OpenShell sandbox provides the security boundary (network policy,
   # credential isolation, binary permissions). Codex's built-in sandbox
   # is redundant and incompatible with the container environment, so we
   # disable it with --dangerously-bypass-approvals-and-sandbox.
   openshell sandbox exec -n "codex-${USER_ID}" -- bash -c '
   codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox \
     "'"${QUESTION}"'"
   '
   ```

   > **Note on `--dangerously-bypass-approvals-and-sandbox`:** The flag
   > name is alarming, but it's the intended mode for externally-sandboxed
   > environments. Codex's built-in bubblewrap sandbox cannot create user
   > namespaces inside the OpenShell container, and its interactive approval
   > prompts don't work in non-interactive `codex exec` mode. The OpenShell
   > sandbox already enforces network policy (only declared endpoints are
   > reachable), binary permissions, and credential isolation — the real
   > security boundary. Disabling Codex's inner sandbox removes the
   > redundant layer that would otherwise block execution.

**Traffic flow:**

```
Codex (in sandbox)
  → inference.local/v1 (model calls)
    → OpenShell privacy router
      → strips credentials, injects real API key
      → forwards to your LLM provider
  → ${SERVER_NAME}:8000/mcp (tool calls)
    → Authorization: Bearer $USER_ACCESS_TOKEN
      → supervisor resolves placeholder to real Keycloak token
      → Envoy checks JWT + realm role → app
```

**Now repeat with user2.** User2 is authorized for `mcp-server-b` (the
Compatibility Engine — tax calculation). Set the variables and run
steps 3-4 again:

```bash
USER_ID="user2"
SERVER_NAME="mcp-server-b"
QUESTION="I live in Lysmark. What is the tax liability for an income of 90000?"
```

Confirm user2's sandbox can reach `mcp-server-b` (`200`) but **not**
`mcp-server-a` (`403`), proving that the per-user credential isolation
works end to end through the agentic coding tool.

Alternatively, run the isolation verification script to test all four
user/server combinations automatically:

```bash
./scripts/08-verify-isolation.sh
```

Expected output:

```
PASS  user1 → mcp-server-a (evaluate_unpaid_leave_eligibility)  HTTP 200 (expected 200)
PASS  user1 → mcp-server-b  HTTP 403 (expected 403)
PASS  user2 → mcp-server-a  HTTP 403 (expected 403)
PASS  user2 → mcp-server-b (calc_tax)  HTTP 200 (expected 200)

Results: 4 passed, 0 failed
```

#### Alternative: Claude Code + BYO LLM + MCP tool

> **Requires an Anthropic Messages API endpoint.** Claude Code uses the
> Anthropic Messages API format, not OpenAI. This recipe only works if your
> LLM provider exposes an Anthropic-compatible endpoint (e.g.
> DeepSeek's `https://api.deepseek.com/anthropic`, or a LiteLLM proxy
> configured with an `/anthropic` route). Standard OpenAI-compatible
> endpoints (vLLM, OpenAI, etc.) will **not** work — use the Codex recipe
> above instead.
>
> **DeepSeek note:** the Anthropic-compatible endpoint uses a different
> base URL (`https://api.deepseek.com/anthropic`) than the OpenAI endpoint
> (`https://api.deepseek.com`). Both use model name `deepseek-v4-flash`
> (or `deepseek-v4-pro`) and the same API key. See `.env.example` for the
> correct values.

Claude Code (pre-installed in the base sandbox image) calling
`mcp-server-a`'s tool (`evaluate_unpaid_leave_eligibility`) via an
Anthropic-compatible LLM endpoint.

**Prerequisites** beyond steps 1-5 above — set `ANTHROPIC_API_KEY`,
`ANTHROPIC_BASE_URL`, and `ANTHROPIC_MODEL` in your `.env` (see
`.env.example`), then in your terminal:

```bash
source .env
USER_ID="user1"
SERVER_NAME="mcp-server-a"
QUESTION="My mother is at the hospital, can I get an aid while I am on unpaid leave?"
LLM_HOST=$(echo "$ANTHROPIC_BASE_URL" | sed 's|https\?://||;s|/.*||')
```

1. Import the Claude Code provider profile and create the provider.

   The profile ([`providers/byo-claude-profile.yaml`](providers/byo-claude-profile.yaml))
   tells OpenShell to inject your LLM API key into the sandbox as
   `ANTHROPIC_API_KEY` — the environment variable Claude Code expects.
   It also includes an endpoint binding for `<llm-host>` (your LLM
   provider's hostname), which must be substituted before import — in
   0.0.106, the proxy only injects credentials for matching endpoints.
   Base URL and model name are passed via `--env` at exec time (step 3),
   since OpenShell only injects **credentials**, not config values.

   Substitute the LLM host placeholder, import, and create the provider:

   ```bash
   TMPFILE=$(mktemp --suffix=.yaml)
   sed "s/<llm-host>/${LLM_HOST}/" providers/byo-claude-profile.yaml > "$TMPFILE"
   openshell provider profile import -f "$TMPFILE"
   rm -f "$TMPFILE"

   openshell provider create --name byo-claude --type byo-claude \
     --credential "ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY"
   ```

2. Attach the provider and grant network access:

   ```bash
   openshell sandbox provider attach "demo-${USER_ID}" byo-claude
   openshell policy update "demo-${USER_ID}" \
     --add-endpoint "${LLM_HOST}:443:read-write:rest:enforce" \
     --binary /usr/local/bin/claude \
     --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
     --binary /usr/local/bin/claude \
     --wait
   ```

3. Run the test. The provider injects `ANTHROPIC_API_KEY` automatically.
   Base URL and model overrides are non-secret config, so we pass them via
   `--env` — OpenShell only injects **credentials** as environment
   variables, not `--config` values:

   ```bash
   openshell sandbox exec -n "demo-${USER_ID}" \
     --env "ANTHROPIC_BASE_URL=$ANTHROPIC_BASE_URL" \
     --env "ANTHROPIC_MODEL=$ANTHROPIC_MODEL" \
     --env "ANTHROPIC_DEFAULT_OPUS_MODEL=$ANTHROPIC_MODEL" \
     --env "ANTHROPIC_DEFAULT_SONNET_MODEL=$ANTHROPIC_MODEL" \
     --env "ANTHROPIC_DEFAULT_HAIKU_MODEL=$ANTHROPIC_MODEL" \
     -- bash -c '
   MCP_JSON="{\"mcpServers\":{\"eligibility\":{\"type\":\"http\",\"url\":\"http://'"${SERVER_NAME}.${OPENSHELL_NAMESPACE}"'.svc.cluster.local:8000/mcp\",\"headers\":{\"Authorization\":\"Bearer $USER_ACCESS_TOKEN\"}}}}"
   claude -p "'"${QUESTION}"'" \
     --mcp-config "$MCP_JSON" \
     --strict-mcp-config \
     --permission-mode bypassPermissions \
     --output-format text
   '
   ```

**Now repeat with user2.** User2 is authorized for `mcp-server-b` (the
Compatibility Engine — tax calculation). Set the variables and run
steps 2-3 again:

```bash
USER_ID="user2"
SERVER_NAME="mcp-server-b"
QUESTION="I live in Lysmark. What is the tax liability for an income of 90000?"
```

Confirm user2's sandbox can call `mcp-server-b`'s `calc_tax` tool
successfully, proving that the per-user credential isolation works end to
end through Claude Code.

#### Red-team evaluation with EvalHub + Garak

Run repeatable, auditable adversarial evaluations against agents running
inside OpenShell sandboxes. EvalHub orchestrates evaluations, Garak probes
for vulnerabilities, and `agent-proxy` (a small Rust server) bridges Garak's
OpenAI-compatible API to the CLI-based agent inside the sandbox.

The proxy runs **inside** the sandbox — Garak's adversarial probes hit the
agent in the exact same environment a real user would have (network policies,
binary permissions, MCP RBAC). See
[`docs/evalhub-redteam.md`](docs/evalhub-redteam.md) for architecture,
design decisions, roles/responsibilities, and background.

> **Note:** In production, red-team evaluation is a secops function, run
> automatically by an automation/service-account identity — never a human,
> and never the target user themselves (self-auditing is a conflict of
> interest). **`user1` here names *whose provider and MCP roles get
> attached to the sandbox*, not who issues the commands.** Every command
> below — sandbox creation, provider attachment, policy updates, proxy
> startup, eval submission — runs from **one continuous admin/secops
> session**. This works because Providers v2 injects credentials per
> *attached* provider and MCP RBAC checks whatever JWT that provider
> supplies at request time — neither cares who ran `sandbox create`. So the
> sandbox gets exactly `user1`'s security context (their MCP roles, their
> credentials) even though an admin/secops identity created and drove it
> end to end. See [`docs/evalhub-redteam.md`](docs/evalhub-redteam.md)'s
> "Roles and responsibilities" section for the full production model
> (representative profiles instead of named users, automated loops across
> all of them).

##### Prerequisites (admin, one-time)

1. **Enable the TrustyAI component** — EvalHub is deployed by
   the TrustyAI Operator, which ships with RHOAI but is disabled by
   default. Enable it in the DataScienceCluster, then wait for the
   operator and CRDs to appear:

   ```bash
   # Enable TrustyAI (skip if already Managed)
   oc patch datasciencecluster default-dsc --type=merge \
     -p '{"spec":{"components":{"trustyai":{"managementState":"Managed"}}}}'

   # Wait for the TrustyAI operator pod
   oc -n redhat-ods-applications get pods -l app=trustyai-operator --watch
   # Ctrl-C once a pod shows Running

   # Confirm the EvalHub CRD exists
   oc api-resources | grep -i eval
   ```

   Also confirm that KServe is in `RawDeployment` mode (required by
   EvalHub):

   ```bash
   oc get datasciencecluster default-dsc \
     -o jsonpath='{.spec.components.kserve.rawDeploymentServiceConfig}'
   # Should print "Headless"
   ```

2. **Deploy PostgreSQL** — EvalHub needs a PostgreSQL database.
   Deploy one in the EvalHub namespace (or point to an existing instance).
   A minimal ephemeral deployment for demo purposes:

   ```bash
   EVALHUB_NAMESPACE="evalhub"
   oc create namespace "$EVALHUB_NAMESPACE" 2>/dev/null || true

   oc -n "$EVALHUB_NAMESPACE" new-app \
     --name=evalhub-db \
     -e POSTGRESQL_USER=evalhub \
     -e POSTGRESQL_PASSWORD=evalhub-demo-password \
     -e POSTGRESQL_DATABASE=evalhub \
     --image-stream="openshift/postgresql:15-el9"

   oc -n "$EVALHUB_NAMESPACE" rollout status deployment/evalhub-db
   ```

3. **Create the EvalHub CR**:

   ```bash
   EVALHUB_NAMESPACE="evalhub"

   # Create the PostgreSQL connection Secret (EvalHub expects a db-url key)
   oc -n "$EVALHUB_NAMESPACE" apply -f - <<'EOF'
   apiVersion: v1
   kind: Secret
   metadata:
     name: evalhub-db-credentials
   type: Opaque
   stringData:
     db-url: "postgresql://evalhub:evalhub-demo-password@evalhub-db:5432/evalhub"
   EOF

   # Create the EvalHub CR with the garak provider
   oc -n "$EVALHUB_NAMESPACE" apply -f - <<'EOF'
   apiVersion: trustyai.opendatahub.io/v1alpha1
   kind: EvalHub
   metadata:
     name: evalhub
   spec:
     replicas: 1
     database:
       type: postgresql
       secret: evalhub-db-credentials
     providers:
       - garak
     collections:
       - safety-and-fairness-v1
   EOF

   # Verify
   oc get pods -l app=eval-hub -n "$EVALHUB_NAMESPACE"
   ```

4. **Install the EvalHub CLI** — use
   [uv](https://docs.astral.sh/uv/) to install into an isolated venv
   without polluting your system Python:

   ```bash
   uv tool install "eval-hub-sdk[cli]"
   evalhub --version
   ```

   Configure the CLI to point at your EvalHub instance:

   ```bash
   EVALHUB_NAMESPACE="evalhub"
   evalhub config set base_url \
     "https://$(oc get routes evalhub -o jsonpath='{.spec.host}' -n "$EVALHUB_NAMESPACE")"
   evalhub config set tenant "$EVALHUB_NAMESPACE"
   evalhub config set token "$(oc whoami -t)"

   # Verify connectivity
   evalhub health
   evalhub providers list
   ```

5. **Deploy `garak-envoy`.** EvalHub's `garak` provider uses the stock
   OpenAI Python client, which sends no custom headers — but
   `openshell service expose` routes purely by HTTP `Host` header (multiple
   sandboxes share one gateway Route). Without a header, a Garak job can
   only ever reach the gateway's *default* vhost — **confirmed on a live
   cluster**: the job fails immediately with `openai.NotFoundError: Error
   code: 404`. `garak-envoy` is a small Envoy proxy that extracts a routing
   key from the request *path* (`/route/<host-key>/...`) instead, and
   rewrites the upstream Host header to it. See
   [`docs/evalhub-redteam.md`](docs/evalhub-redteam.md)'s "CONFIRMED
   BROKEN" note for the full root-cause writeup. Deploy it once per
   cluster:

   ```bash
   helm upgrade --install garak-envoy demos/keycloak-oidc/garak-envoy \
     --namespace "$OPENSHELL_NAMESPACE"

   GARAK_ENVOY_HOST=$(oc get route garak-envoy -n "$OPENSHELL_NAMESPACE" \
     -o jsonpath='{.spec.host}')
   echo "garak-envoy route: $GARAK_ENVOY_HOST"
   ```

6. **Get the `agent-proxy` static binary.** No Rust toolchain needed —
   download the prebuilt musl binary from GitHub Releases into the same
   path a local build would produce, so the `AGENT_PROXY_BIN` variable
   used below works either way:

   ```bash
   AGENT_PROXY_DIR="../../util/agent-proxy/target/x86_64-unknown-linux-musl/release"
   mkdir -p "$AGENT_PROXY_DIR"
   curl -fsSL -o "$AGENT_PROXY_DIR/agent-proxy" \
     https://github.com/alpha-hack-program/openshell-demos/releases/latest/download/agent-proxy-linux-x86_64-musl
   chmod +x "$AGENT_PROXY_DIR/agent-proxy"
   ```

   If you're actively developing `agent-proxy` (or no release exists yet),
   build from source instead — requires Rust 2024 edition (1.85+) and the
   `x86_64-unknown-linux-musl` target. See
   [`util/agent-proxy/README.md`](../../util/agent-proxy/README.md) for the
   full build/release/image workflow:

   ```bash
   make -C ../../util/agent-proxy musl
   ```

   Always use the musl target for anything deployed into a sandbox — a
   plain `cargo build --release` binary links against whatever glibc your
   dev machine has, which is typically too new (confirmed: Fedora 44's
   glibc 2.41) for the sandbox base images.

7. **Enable MLflow result tracking (optional but recommended).** EvalHub
   has no native RHOAI dashboard view of its own (the dashboard's model
   evaluation UI is LM-Eval-only) — MLflow is the only UI-capable path for
   Garak results. If this cluster already has the native RHOAI MLflow
   instance (check: `oc get mlflow -A`), the TrustyAI operator has likely
   already pre-wired most of the integration (CA cert, workspace, and a
   projected-ServiceAccount-token mount with matching RBAC) — the **only**
   thing usually missing is `MLFLOW_TRACKING_URI`, which is what actually
   enables tracking:

   ```bash
   EVALHUB_NAMESPACE="evalhub"
   MLFLOW_INTERNAL_URL=$(oc get mlflow mlflow -n redhat-ods-applications \
     -o jsonpath='{.status.address.url}')

   oc patch evalhub evalhub -n "$EVALHUB_NAMESPACE" --type=merge -p \
     "{\"spec\":{\"env\":[{\"name\":\"MLFLOW_TRACKING_URI\",\"value\":\"${MLFLOW_INTERNAL_URL}\"}]}}"

   oc rollout status deployment/evalhub -n "$EVALHUB_NAMESPACE"
   ```

   This redeploys the EvalHub server, which cancels any job currently
   running — do this before submitting evaluations, not mid-run. Once
   healthy again (`evalhub health`), pass `--experiment <name>` on job
   submissions (see step 5 below) to log results to MLflow. See
   [`docs/evalhub-redteam.md`](docs/evalhub-redteam.md)'s "Viewing results
   in the RHOAI dashboard / MLflow" section for the full writeup, including
   how to query results directly (RHOAI's MLflow requires a
   `X-MLflow-Workspace` header on every API call — a detail the plain web
   UI may also need if experiments don't show up).

##### Demo steps (admin/secops session, targeting user1's context) — Claude Code, recommended

Run these — from the **same admin/secops session as the Prerequisites
above** — after completing steps 1-5 of the main demo. `user1` must
already have their provider and MCP roles configured (steps 3-4 of the
main demo); nothing here requires logging in *as* user1. **Claude Code is
the recommended agent for this section** — its Anthropic Messages API
works fully (including MCP tool use) against this demo's DeepSeek BYO
backend. The Codex variant below only supports model-only probes here (see
why in that variant's intro).

```bash
source .env
source ../../.env
USER_ID="user1"
SANDBOX="garak-claude-${USER_ID}"
CLAUDE_IMAGE="quay.io/aipcc/agentic-ci/claude-sandbox:0.3.36"
AGENT_PROXY_BIN="../../util/agent-proxy/target/x86_64-unknown-linux-musl/release/agent-proxy"
LLM_HOST=$(echo "$ANTHROPIC_BASE_URL" | sed 's|https\?://||;s|/.*||')
SERVER_NAME="mcp-server-a"
# Re-derive if this is a fresh shell from the Prerequisites section:
GARAK_ENVOY_HOST=$(oc get route garak-envoy -n "$OPENSHELL_NAMESPACE" -o jsonpath='{.spec.host}')
```

**1. Create the `byo-claude` provider** (skip if you already created it for
the "Claude Code + BYO LLM + MCP tool" recipe above):

```bash
TMPFILE=$(mktemp --suffix=.yaml)
sed "s/<llm-host>/${LLM_HOST}/" providers/byo-claude-profile.yaml > "$TMPFILE"
openshell provider profile import -f "$TMPFILE"
rm -f "$TMPFILE"

openshell provider create --name byo-claude --type byo-claude \
  --credential "ANTHROPIC_API_KEY=$ANTHROPIC_API_KEY"
```

**2. Create a sandbox from the stock Claude Code image, attach providers,
and grant network access:**

```bash
openshell sandbox create --name "$SANDBOX" --from "$CLAUDE_IMAGE" -- true

openshell sandbox provider attach "$SANDBOX" byo-claude
openshell sandbox provider attach "$SANDBOX" "user-${USER_ID}"

openshell policy update "$SANDBOX" \
  --add-endpoint "${LLM_HOST}:443:read-write:rest:enforce" \
  --binary /usr/local/bin/claude \
  --add-endpoint "${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000:read-write:rest:enforce" \
  --binary /usr/local/bin/claude \
  --wait
```

**3. Upload agent-proxy and an MCP-config wrapper script.** Claude Code's
`--mcp-config` JSON must be built at *runtime* inside the sandbox, since it
embeds `$USER_ACCESS_TOKEN` — credential resolution happens at the network
layer when `claude` sends this exact placeholder string to a
policy-matched endpoint, not when a shell expands the variable (confirmed:
a plain `bash -c 'echo $USER_ACCESS_TOKEN'` prints the literal placeholder,
never the real token). A wrapper script is required because agent-proxy's
`AGENT_COMMAND` is split naively on whitespace — it can't express the
shell quoting this needs:

```bash
cat > /tmp/run-claude.sh << EOF
#!/bin/bash
set -e
MCP_JSON="{\\"mcpServers\\":{\\"eligibility\\":{\\"type\\":\\"http\\",\\"url\\":\\"http://${SERVER_NAME}.${OPENSHELL_NAMESPACE}.svc.cluster.local:8000/mcp\\",\\"headers\\":{\\"Authorization\\":\\"Bearer \$USER_ACCESS_TOKEN\\"}}}}"
exec claude -p "\$1" \\
  --mcp-config "\$MCP_JSON" \\
  --strict-mcp-config \\
  --permission-mode bypassPermissions \\
  --output-format text
EOF

openshell sandbox upload "$SANDBOX" "$AGENT_PROXY_BIN" /sandbox/agent-proxy
openshell sandbox exec -n "$SANDBOX" -- chmod +x /sandbox/agent-proxy
openshell sandbox upload "$SANDBOX" /tmp/run-claude.sh /sandbox/run-claude.sh
openshell sandbox exec -n "$SANDBOX" -- chmod +x /sandbox/run-claude.sh
```

**4. Start the proxy in background and expose the service.** Claude Code's
`-p` mode doesn't need a real TTY (unlike Codex — see the Codex variant
below), so a plain background start is fine:

```bash
nohup openshell sandbox exec -n "$SANDBOX" \
  --env 'AGENT_COMMAND=/sandbox/run-claude.sh' \
  --env 'OUTPUT_FILE_FLAG=' \
  --env "ANTHROPIC_BASE_URL=$ANTHROPIC_BASE_URL" \
  --env "ANTHROPIC_MODEL=$ANTHROPIC_MODEL" \
  --env "ANTHROPIC_DEFAULT_OPUS_MODEL=$ANTHROPIC_MODEL" \
  --env "ANTHROPIC_DEFAULT_SONNET_MODEL=$ANTHROPIC_MODEL" \
  --env "ANTHROPIC_DEFAULT_HAIKU_MODEL=$ANTHROPIC_MODEL" \
  -- /sandbox/agent-proxy --port 8100 > /tmp/agent-proxy-exec.log 2>&1 &
PROXY_EXEC_PID=$!

openshell service expose "$SANDBOX" 8100
```

Verify the proxy is reachable and MCP tool use works — **validated on a
live cluster**: the eligibility engine returned a real, tool-derived
answer (not a hallucination), confirmed against `mcp-server-a`'s own
container logs (`called_by`/`roles` claims matched the authenticated
user's real JWT):

```bash
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
SERVICE_HOST="default--${SANDBOX}.openshell.localhost"

curl -sk -X POST "https://${ROUTE_HOST}/v1/chat/completions" \
  -H "Host: ${SERVICE_HOST}" \
  -H "Content-Type: application/json" \
  -d '{"messages":[{"role":"user","content":"My mother is at the hospital, can I get an aid while I am on unpaid leave?"}]}' | jq .
```

**5. Submit an EvalHub evaluation through `garak-envoy`.** Point
`--model-url` at `garak-envoy`'s Route with the sandbox's Host-header key
embedded in the path — **confirmed working end-to-end on a live cluster**
(the `quick` and `owasp_llm_top10` benchmarks both completed with real
metrics). Add `--experiment` if you completed the optional MLflow step in
the Prerequisites above:

```bash
evalhub eval run \
  --name "redteam-${USER_ID}" \
  --model-url "https://${GARAK_ENVOY_HOST}/route/${SERVICE_HOST}" \
  --model-name "${SANDBOX}-agent-proxy" \
  --provider garak \
  -b quick \
  --experiment "redteam-${USER_ID}"
```

**6. Track results:**

```bash
evalhub eval status <job_id>
evalhub eval results <job_id>
```

If MLflow tracking is enabled, the job response includes a
`mlflow_experiment_id` and (once complete) each benchmark result includes
an `mlflow_run_id` — see
[`docs/evalhub-redteam.md`](docs/evalhub-redteam.md)'s "Viewing results in
the RHOAI dashboard / MLflow" section to browse or query them.

**7. Cleanup:**

```bash
kill "$PROXY_EXEC_PID" 2>/dev/null
openshell service delete "$SANDBOX"
openshell sandbox delete "$SANDBOX"
```

##### Demo steps (admin/secops session, targeting user1's context) — Codex variant, optional

Codex requires an on-cluster vLLM **≥0.25.0** (upstream — RHOAI 3.4.x
ships 0.18.0, too old) for MCP tool use; its `namespace` tool type isn't
supported by this demo's DeepSeek BYO backend (confirmed: DeepSeek rejects
it with a 400). Without that endpoint, Codex can still run **model-only**
red-team probes (no MCP tool calls). Use this variant only if you have
such a vLLM endpoint, or specifically want to red-team the model in
isolation. Same session model as the Claude Code steps above — nothing
here requires logging in as user1.

```bash
source .env
source ../../.env
USER_ID="user1"
SANDBOX="garak-codex-${USER_ID}"
AGENT_IMAGE="quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012"
AGENT_PROXY_BIN="../../util/agent-proxy/target/x86_64-unknown-linux-musl/release/agent-proxy"
GARAK_ENVOY_HOST=$(oc get route garak-envoy -n "$OPENSHELL_NAMESPACE" -o jsonpath='{.spec.host}')
```

**1. Create the sandbox and attach providers:**

```bash
openshell sandbox create --name "$SANDBOX" --from "$AGENT_IMAGE" -- true
openshell sandbox provider attach "$SANDBOX" "user-${USER_ID}"
openshell sandbox provider attach "$SANDBOX" byo-codex
```

**2. Upload the agent-proxy binary:**

```bash
openshell sandbox upload "$SANDBOX" "$AGENT_PROXY_BIN" /sandbox/agent-proxy
openshell sandbox exec -n "$SANDBOX" -- chmod +x /sandbox/agent-proxy
```

**3. Start the proxy in the FOREGROUND with `--tty`, backgrounded on the
*local* machine.** Codex's `exec` subcommand refuses to run
non-interactively unless stdin, stdout, AND stderr are all real TTYs and
`TERM` isn't `dumb` — even with `--dangerously-bypass-approvals-and-sandbox`.
`sandbox exec` only allocates a pty when `--tty` is passed explicitly (or
the calling terminal is itself a real tty) — a background
`nohup agent-proxy &` *inside* the sandbox never requests one, and tears
down the exec channel (and its pty) the moment the wrapping shell exits
regardless. See [`docs/evalhub-redteam.md`](docs/evalhub-redteam.md)'s "TTY
root cause" section for the full investigation:

```bash
nohup openshell sandbox exec -n "$SANDBOX" --tty \
  --env 'AGENT_COMMAND=codex exec --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox' \
  -- /sandbox/agent-proxy --port 8100 > /tmp/agent-proxy-exec.log 2>&1 &
PROXY_EXEC_PID=$!

openshell service expose "$SANDBOX" 8100
```

**4. Verify, submit the eval, track results, and clean up** — same as the
Claude Code steps 4-7 above, substituting this variant's `$SANDBOX` (the
model-only probe results won't include MCP-related findings).

See [`docs/evalhub-redteam.md`](docs/evalhub-redteam.md) for the custom
image approach (Approach B), the automated evaluation loop across
representative user profiles, and the deeper risk assessment path (Path 2
with KFP + custom harm categories).

## Configuration reference

| Variable | Where used | Notes |
|---|---|---|
| `KEYCLOAK_HOST` | Helm overlay, provider profiles | e.g. `keycloak.apps.<cluster-domain>` |
| `KEYCLOAK_REALM` | All Keycloak-facing config | `openshell` in this demo |
| `KEYCLOAK_CLIENT_ID_CLI` | `server.oidc.audience` | Must match the Keycloak client ID exactly |
| `KEYCLOAK_CLIENT_ID_GATEWAY` | Confidential gateway client | Used for gateway-to-Keycloak communication |
| `KEYCLOAK_CLIENT_SECRET` | Gateway client secret | Never commit a real value |
| `KEYCLOAK_ADMIN_TOKEN` | `07-authorize-mcp-user.sh` | Short-lived; obtain via your own admin login |

## Secrets and security notes

- The gateway client secret in `keycloak/realm-export.json` is a hardcoded
  demo value (`openshell-gateway-demo-secret`). In production, generate a
  unique secret per environment and never commit it to git.
- `openshell provider refresh configure` supports `--secret-material-key` to
  mark values as sensitive at the gateway — used for `refresh_token` in the
  onboarding commands. No `client_secret` is needed because the refresh token
  is bound to the public CLI client (`openshell-cli`), which has no secret.
- Each user's provider uses a distinct name (`user-<id>`). Providers v2
  rejects two providers on one sandbox that expose the same credential
  environment key, which catches naming collisions — but not *misassignment*.
  OpenShell has no built-in concept of "this sandbox belongs to this user";
  getting the right provider attached to the right sandbox is entirely this
  demo's orchestration responsibility.
- The gateway defaults to TLS with mTLS client authentication (see
  [`demos/base/README.md`](../base/README.md) for background). Once this
  demo's values are applied, real OIDC auth is enforced
  (`allowUnauthenticatedUsers: false`), but the transport is still plaintext
  — evaluation-only, never expose to a public network.

## Troubleshooting

**Profile `token_url` substitution.** The provider profile
`providers/user-refresh-profile.yaml` contains `<keycloak-host>` as a
placeholder in `token_url`. This must be replaced before import. If you
imported a profile with the wrong URL, you must delete and recreate any
providers built from it — a plain `profile update` is not enough because
providers snapshot the profile state at creation time.

**`--from-oidc-token` binds to the CLI's own session.** The `--from-oidc-token`
flag on `openshell provider create` binds to the CLI's own current OIDC
session, not an arbitrary token you hand it. The onboarding commands in this
guide use the general `--credential`/refresh-material mechanism instead.

**Envoy sidecar is required for MCP server access control.** The MCP server
images themselves do not enforce the Keycloak role check — tested and
confirmed: requests with no token, garbage tokens, and valid tokens lacking
the required role all reach the tool identically. Do not remove the Envoy
sidecar on the assumption the app image checks anything.

**Envoy JWKS TLS.** The Envoy sidecar's connection to Keycloak's JWKS
endpoint uses `trust_chain_verification: ACCEPT_UNTRUSTED` (skips CA
validation). This matches the demo's `curl -k` pattern — fine for evaluation,
not for production.

**Envoy image tag.** `envoyproxy/envoy:v1.31-latest` is a moving tag. Pin an
exact patch release before relying on this beyond a demo.

## Definition of done

- [ ] Keycloak realm `openshell` live with CLI and gateway clients, admin/user roles
- [ ] OIDC overlay applied; `openshell status` shows the CLI authenticated against Keycloak
- [ ] RBAC mode confirmed: a user-role token cannot perform admin-only operations
- [ ] Providers v2 enabled
- [ ] At least two demo users onboarded, each with their own provider
- [ ] Isolation test passes: user A's sandbox cannot access user B's data
      even when both sandboxes run concurrently
- [x] (Stretch) `mcp-servers` chart deployed; a user holding the required
      Keycloak role can reach their MCP server, a user lacking it cannot
      — verified via the Envoy sidecar (401/403/200 cases all tested live)
- [x] (Stretch) A user authorized for one MCP server's role does not
      thereby gain access to the other — verified both directions
- [x] (Stretch) Codex variant — verified with Codex 0.146.0 + vLLM 0.27.1 + MCP server 3.1.5

## Open risks

- **This README is a reconstruction, not a transcription** of NVIDIA's own
  examples. Reconcile every command against the real repo before running it.
- **Provider profile schema** — verified against
  [Providers v2 docs](https://docs.nvidia.com/openshell/sandboxes/providers-v2)
  and a live gateway (CLI 0.0.106). `refresh` (with `token_url`, `scopes`,
  `strategy`) must nest under the specific entry in `credentials[]`, not as a
  top-level profile field.
- **Real user identity federation** (brokering each user's own IdP into
  Keycloak) is a materially bigger project than this demo covers.
- **Per-server token audience** — Keycloak isn't configured with an audience
  mapper per MCP server, so the realm role claim is the *only* thing
  distinguishing access to server A from server B.

## Experimental future work: Path B — SPIRE/SPIFFE token exchange

> **Nothing in this section has ever been deployed or tested.** All commands
> are **[VERIFY]**.

### What Path B would do differently

Instead of storing a user's refresh token and having the gateway refresh it
directly, Path B uses SPIFFE workload identity:

1. SPIRE issues JWT-SVIDs to both the sandbox supervisor and the gateway.
2. The sandbox presents its SVID to the gateway when requesting a provider
   token.
3. The gateway authenticates to Keycloak using its own SVID as a client
   assertion (RFC 7523) and asks for a token exchange (RFC 8693).

This removes the need for a long-lived refresh token per user but requires
SPIRE infrastructure and Keycloak's SPIFFE federated client authentication.

### Prerequisites (beyond the current demo)

- SPIRE server + agent deployed on the cluster
- SPIFFE CSI driver for workload identity injection
- `server.providerTokenGrants.spiffe.enabled=true` in the Helm overlay
- Keycloak token-exchange feature enabled
- Keycloak federated client authentication configured to trust the SPIRE
  trust domain's bundle endpoint

### Steps (all [VERIFY])

```bash
./scripts/04-deploy-spire.sh
./scripts/05-register-spire-entries.sh
```

### Open risks specific to Path B

- **Provider profile schema is unconfirmed.** `token_exchange` is not among
  the five documented refresh strategies. The shape in
  `providers/token-exchange-profile.yaml` is inferred from a GitHub design
  discussion (issue #1987).
- **Nothing in this repo has ever gotten SPIRE running on this cluster.**

### References (Path B)

- Dynamic token grant design discussion: https://github.com/NVIDIA/OpenShell/issues/1987
- Keycloak SPIFFE federated client auth: https://www.keycloak.org/2026/01/federated-client-authentication
- Keycloak SPIFFE playground demo: https://github.com/keycloak/keycloak-playground/tree/main/federated-client-authentication/spiffe
- SPIRE Kubernetes quickstart: https://spiffe.io/docs/latest/try/getting-started-k8s/

## References

- OpenShift install path: https://docs.nvidia.com/openshell/kubernetes/openshift
- Access Control / OIDC: https://docs.nvidia.com/openshell/kubernetes/access-control
- Providers v2: https://docs.nvidia.com/openshell/sandboxes/providers-v2
- Manage Providers: https://docs.nvidia.com/openshell/sandboxes/manage-providers
- Helm chart README: https://github.com/NVIDIA/OpenShell/blob/main/deploy/helm/openshell/README.md
- OpenShift SCC restriction discussion: https://github.com/NVIDIA/OpenShell/issues/899
