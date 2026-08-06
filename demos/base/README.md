# base — OpenShell on OpenShift, generic install + hello world

Demo-agnostic. Installs the OpenShell gateway on OpenShift and proves the
install works with a minimal sandbox test. Other demos carry their own complete `helm/values.yaml` (including these
same OpenShift overrides) so they can be installed independently.

## Prerequisites

| Tool / access | Notes |
|---|---|
| `oc` | Logged into the target cluster, with rights to grant SCCs |
| `helm` 3.x | |
| `kubectl` | Compatible with cluster version |
| `openshell` CLI | See [Installing the CLI](#installing-the-cli) below |
| OpenShift 4.x cluster | |
| Agent Sandbox controller + CRDs | See [Installing Agent Sandbox](#installing-agent-sandbox) below — must be done **before** `helm install` |
| cert-manager Operator *(optional)* | If installed, you can use it for TLS instead of the built-in PKI init job — see [Choosing how TLS certificates are generated](#choosing-how-tls-certificates-are-generated) |

You can run these tools from any machine that can reach the cluster: your
laptop, a jump host, a VM, a container. The repo includes a
[Vagrantfile](../../Vagrantfile) that provisions a Fedora VM with everything
pre-installed if you want a self-contained Linux workstation — see
[Using the Vagrant VM](#using-the-vagrant-vm) at the bottom. But it's
entirely optional.

### Installing the CLI

On **Fedora/RHEL** x86_64, install the RPM directly from the GitHub release:

```bash
OPENSHELL_VERSION="0.0.97"   # match OPENSHELL_CHART_VERSION in .env
sudo dnf install -y \
  "https://github.com/NVIDIA/OpenShell/releases/download/v${OPENSHELL_VERSION}/openshell-${OPENSHELL_VERSION}-1.fc44.x86_64.rpm"
openshell --version
```

On **macOS** (Apple Silicon):

```bash
curl -sL "https://github.com/NVIDIA/OpenShell/releases/download/v${OPENSHELL_VERSION}/openshell-aarch64-apple-darwin.tar.gz" \
  | tar xzf - -C /usr/local/bin
openshell --version
```

Other assets (musl tarball, aarch64 Linux, `.deb`, `.snap`) are listed at
https://github.com/NVIDIA/OpenShell/releases.

> **Note:** the GitHub release *tag* uses a `v` prefix (`v0.0.97`) but the
> Helm chart version does **not** (`0.0.97`). `OPENSHELL_CHART_VERSION` in
> `.env` must be set without the `v` — e.g. `OPENSHELL_CHART_VERSION=0.0.97`.

#### Bash completions

```bash
# system-wide (requires root)
sudo sh -c 'openshell completions bash > /etc/bash_completion.d/openshell'

# or per-user
mkdir -p ~/.local/share/bash-completion/completions
openshell completions bash > ~/.local/share/bash-completion/completions/openshell
```

Restart your shell (or `source ~/.bashrc`) to activate. Also available for
`zsh`, `fish`, and `powershell` — run `openshell completions --help`.

### Installing Agent Sandbox

The Agent Sandbox controller and CRDs come from the
[kubernetes-sigs/agent-sandbox](https://github.com/kubernetes-sigs/agent-sandbox)
project. Install them **before** the OpenShell Helm chart:

```bash
# Latest
kubectl apply -f https://github.com/kubernetes-sigs/agent-sandbox/releases/latest/download/sandbox.yaml

# Or pin a version
VERSION="v0.5.4"
kubectl apply -f "https://github.com/kubernetes-sigs/agent-sandbox/releases/download/${VERSION}/sandbox.yaml"
```

Verify the controller is running:

```bash
kubectl -n agent-sandbox-system get pods
# NAME                                       READY   STATUS    AGE
# agent-sandbox-controller-xxxxx             1/1     Running   ...
```

> **Gotcha:** the manifest file is called `sandbox.yaml`, **not**
> `manifest.yaml`. The release also offers `sandbox-with-extensions.yaml`
> (adds SandboxTemplate, SandboxClaim, SandboxWarmPool CRDs).

## How OpenShell networking works

The OpenShell CLI talks to the gateway over **gRPC**, which runs natively on
**HTTP/2**. This has direct consequences for how you expose the gateway:

```
openshell CLI
  └── gRPC  (application: messages, streaming, status codes)
       └── HTTP/2  (transport: multiplexed streams, binary framing)
            └── TLS  (encryption + optional mTLS client auth)
                 └── TCP
```

gRPC requires HTTP/2 — it cannot fall back to HTTP/1.1. A standard OpenShift
**edge** or **re-encrypt** Route terminates TLS and re-originates the
backend connection as HTTP/1.1, which breaks gRPC. To expose the gateway
externally you need one of:

| Approach | How it works |
|---|---|
| `oc port-forward` | Tunnels raw TCP from localhost to the pod — no Route involved |
| **Passthrough Route** | Passes raw TLS through to the pod; the gateway terminates TLS itself, preserving HTTP/2 end-to-end |
| **Envoy Gateway** | NVIDIA's recommended production path — Envoy natively supports HTTP/2 and gRPC proxying |

### Authentication layers

The gateway has two independent authentication checks:

| Layer | Mechanism | What provides it |
|---|---|---|
| **Transport** | mTLS — server verifies client cert, client verifies server cert | PKI init job generates both sides; the CLI sends the client cert from `~/.config/openshell/gateways/<name>/mtls/` |
| **Application** | gRPC `authorization` header carrying a JWT | OIDC login (`openshell gateway login`), or an edge proxy that injects tokens |

With both layers active, a request must present a valid client certificate
**and** a valid JWT. In this base install we use mTLS only and set
`allowUnauthenticatedUsers=true` to skip the JWT check — the transport is
still encrypted and client-authenticated, but there's no second token layer.
The `keycloak-oidc` demo adds Keycloak as an OIDC provider, enabling
the JWT layer as well.

## What this installs

- OpenShell gateway (StatefulSet) in `$OPENSHELL_NAMESPACE`
- TLS-enabled gateway with mTLS client authentication — the PKI init job
  generates server certs, client certs, and sandbox JWT signing keys
  automatically

`helm/values-openshift.yaml`:

```yaml
podSecurityContext:
  fsGroup: null
securityContext:
  runAsUser: null
```

The only overrides are `fsGroup` and `runAsUser` set to `null` — OpenShift's
admission controller must assign these itself. Everything else uses chart
defaults: TLS enabled, mTLS for client auth, PKI init job generates all
certificates.

After `helm install`, extract the client mTLS bundle so the CLI can
authenticate:

```bash
MTLS_DIR=~/.config/openshell/gateways/openshift/mtls
mkdir -p "$MTLS_DIR"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"
```

> **Why TLS instead of plaintext?** See
> [How OpenShell networking works](#how-openshell-networking-works) for
> background on the protocol stack and auth layers. The
> [official OpenShift install guide](https://docs.nvidia.com/openshell/kubernetes/openshift)
> sets `server.disableTls=true` because it pairs with Envoy Gateway for TLS
> termination at the edge (see the
> [ingress guide](https://docs.nvidia.com/openshell/kubernetes/ingress)).
> That approach also requires `allowUnauthenticatedUsers=true` (or OIDC)
> since there's no client certificate path through Envoy.
>
> We keep TLS enabled because:
> - mTLS provides client authentication out of the box — no need for
>   OIDC configuration in the base install
> - Port-forward works (the server cert SANs include `localhost` / `127.0.0.1`)
> - A passthrough OpenShift Route can expose the gateway externally with
>   gRPC over HTTP/2 — see
>   [Exposing the gateway via passthrough Route](#exposing-the-gateway-via-passthrough-route)
>
> **To follow the official plaintext path instead**, use these values:
>
> ```yaml
> server:
>   disableTls: true
>   auth:
>     allowUnauthenticatedUsers: true
> podSecurityContext:
>   fsGroup: null
> securityContext:
>   runAsUser: null
> ```
>
> Then register the gateway with `http://` (no mTLS extraction needed):
>
> ```bash
> openshell gateway add http://127.0.0.1:8080 --local --name openshift
> ```

## Choosing how TLS certificates are generated

Before running the install, decide who generates the server and client
TLS certificates. The OpenShell Helm chart supports two options:

| | PKI init job (default) | cert-manager Operator |
|---|---|---|
| **How it works** | A Helm pre-install hook Job runs `generate-certs` and creates the TLS secrets | The chart creates `Issuer` + `Certificate` CRs; cert-manager issues the certs |
| **Certificate renewal** | Manual — delete secrets, re-run `helm upgrade` | Automatic — cert-manager renews before expiry |
| **JWT signing keys** | Generated by the init job | **Still** generated by the init job (JWT-only mode) |
| **Extra dependency** | None | OpenShift cert-manager Operator must be installed |
| **Best for** | Quick evaluation, no extra operators needed | Production-like setup, automated rotation |

### Check if cert-manager is available

```bash
oc get csv -A | grep cert-manager
# If you see a row with "Succeeded", the operator is installed.
# If not, the PKI init job path is your only option (or install the
# cert-manager Operator from OperatorHub first).
```

### Enable the cert-manager path

Set `CERT_MANAGER=true` in your `.env`:

```bash
# In demos/base/.env
CERT_MANAGER=true
```

That's it. The install script (`02-install-openshell.sh`) picks up this
toggle and automatically:

- Uses `helm/values-openshift-certmanager.yaml` instead of
  `values-openshift.yaml`.
- Appends your namespace to `certManager.serverDnsNames` via `--set`
  overrides.
- If `OPENSHELL_ROUTE=true`, adds the **route FQDN** to
  `certManager.serverDnsNames` (instead of `pkiInitJob.serverDnsNames`).
  This is critical — without it, the server certificate won't include the
  route hostname and TLS handshakes from clients connecting via the Route
  will fail with a hostname mismatch.

> **If you're using a passthrough Route** (which you likely are — see
> [Exposing the gateway via passthrough Route](#exposing-the-gateway-via-passthrough-route)),
> make sure both `CERT_MANAGER=true` **and** `OPENSHELL_ROUTE=true` are
> set in `.env`. The script will include the route FQDN
> (`openshell-<namespace>.<cluster-apps-domain>`) in the cert-manager
> `Certificate` SANs.

You do **not** need to edit the values file. The script computes all
DNS names from your `.env` variables and passes them as `--set` overrides.
It will print the full list of SANs it computed so you can verify:

```
cert-manager path enabled — TLS certificates will be managed by cert-manager.
  PKI init job will only generate JWT signing keys.
  Server cert SANs (from values file): openshell, 127.0.0.1
  Server cert SANs (computed):         openshell.openshell-base-demo.svc
                                       openshell.openshell-base-demo.svc.cluster.local
                                       openshell-openshell-base-demo.apps.example.com
```

When `certManager.enabled: true`, the chart:

1. Creates a self-signed `Issuer` + root CA `Certificate` (stored in
   `openshell-ca-tls`).
2. Creates a namespaced `Issuer` backed by that CA.
3. Issues `Certificate` resources for the server (`openshell-server-tls`)
   and client (`openshell-client-tls`) TLS secrets.
4. The PKI init job **still runs** but in JWT-only mode — it only
   generates the Ed25519 sandbox JWT signing keys
   (`openshell-jwt-keys`), not TLS material.

> **Do not** set `pkiInitJob.enabled: false` when using cert-manager —
> the JWT signing keys are not managed by cert-manager and will not be
> generated without the init job.

### If you don't have cert-manager

Leave `CERT_MANAGER=false` (the default) or omit it entirely. The
install script will use `values-openshift.yaml` and the PKI init job
generates everything. No extra steps needed.

### Everything after this point is the same

Regardless of which path you choose, the TLS secrets have the same
names and format (`openshell-server-tls`, `openshell-client-tls`). The
mTLS extraction, gateway registration, sandbox creation, and all
subsequent steps are identical.

## Install

### 1. Clone the repo and change to the demo directory

```bash
git clone https://github.com/alpha-hack-program/openshell-demos.git
cd openshell-demos/demos/base
```

### 2. Create your `.env` files

Copy the example files and fill in the real values:

```bash
# Root .env — cluster-wide variables
cp ../../.env.example ../../.env
# Edit ../../.env and set OPENSHELL_CHART_VERSION and CLUSTER_APPS_DOMAIN

# Demo .env — demo-specific variables
cp .env.example .env
# Review .env — defaults are fine for most setups
```

The `.env.example` ships with `OPENSHELL_ROUTE=true` because the **passthrough
Route is the recommended path** for this demo. It exposes the gateway over the
network so you don't need an active `oc port-forward` terminal, and it
preserves HTTP/2 end-to-end for gRPC (see
[How OpenShell networking works](#how-openshell-networking-works)). If you
prefer a local-only setup, set `OPENSHELL_ROUTE=false` in your `.env`.

### 3. Run the install scripts

The scripts expect environment variables to be **exported**, not just sourced.
Source both `.env` files before running:

```bash
export $(grep -v '^#' .env | xargs)
export $(grep -v '^#' ../../.env | xargs)

./scripts/00-prereqs-check.sh
./scripts/01-namespace-and-scc.sh
./scripts/02-install-openshell.sh
./scripts/03-connect-gateway.sh
```

The install and connect scripts automatically handle the route hostname in the
server cert SANs, `allowUnauthenticatedUsers=true`, and Route creation when
`OPENSHELL_ROUTE=true`.

## Exposing the gateway via passthrough Route

This is the **recommended path** and the default in `.env.example`
(`OPENSHELL_ROUTE=true`). A passthrough Route lets you (and others) reach the
gateway over the network without an active `oc port-forward` terminal. If you
left the default, the install scripts already handled this for you — the
manual steps below are only needed if you want to understand what happened or
if you're upgrading an existing install that was initially deployed without a
route.

As explained in [How OpenShell networking works](#how-openshell-networking-works),
gRPC requires HTTP/2 end-to-end, so only a passthrough Route works — it
forwards raw TLS to the pod, which terminates the connection itself.

### 1. Re-deploy with the route hostname in the server cert

The route hostname must be in the server certificate's SANs — without it,
TLS handshakes will fail because the hostname won't match. Pass it as a
`--set` override — don't hardcode it in the values file:

```bash
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
```

**If upgrading an existing install**, delete the TLS secrets first so they
are regenerated with the new SAN:

```bash
oc -n "$OPENSHELL_NAMESPACE" delete secret \
  openshell-server-tls openshell-client-tls openshell-jwt-keys
```

Then install/upgrade with the route hostname in the SANs. The `--set`
flag differs depending on whether you use the PKI init job or
cert-manager:

**PKI init job (default):**

```bash
helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" -n "$OPENSHELL_NAMESPACE" \
  -f demos/base/helm/values-openshift.yaml \
  --set "pkiInitJob.serverDnsNames[0]=${ROUTE_HOST}" \
  --set "server.auth.allowUnauthenticatedUsers=true"
```

**cert-manager:**

```bash
helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" -n "$OPENSHELL_NAMESPACE" \
  -f demos/base/helm/values-openshift-certmanager.yaml \
  --set "certManager.serverDnsNames[2]=openshell.${OPENSHELL_NAMESPACE}.svc" \
  --set "certManager.serverDnsNames[3]=openshell.${OPENSHELL_NAMESPACE}.svc.cluster.local" \
  --set "certManager.serverDnsNames[4]=${ROUTE_HOST}" \
  --set "server.auth.allowUnauthenticatedUsers=true"
```

> **Using the scripts?** Set `OPENSHELL_ROUTE=true` (and
> `CERT_MANAGER=true` if applicable) in `.env` — the install script
> handles all of this automatically.

> **Why `allowUnauthenticatedUsers`?** As described in
> [Authentication layers](#authentication-layers), the gateway checks two
> layers: mTLS (transport) and a JWT authorization header (application).
> Without OIDC there's nothing to provide the JWT. This flag tells the server
> to accept requests authenticated by mTLS alone. It does **not** disable
> TLS or skip client certificate verification.

### 2. Create the passthrough Route

```bash
oc -n "$OPENSHELL_NAMESPACE" create route passthrough openshell \
  --service=openshell \
  --port=8080 \
  --hostname="${ROUTE_HOST}"
```

### 3. Re-extract client certs and register the gateway

After cert regeneration, the client mTLS bundle must be refreshed:

```bash
MTLS_DIR=~/.config/openshell/gateways/openshift/mtls
mkdir -p "$MTLS_DIR"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.ca\.crt}'  | base64 -d > "$MTLS_DIR/ca.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.crt}' | base64 -d > "$MTLS_DIR/tls.crt"
oc -n "$OPENSHELL_NAMESPACE" get secret openshell-client-tls \
  -o jsonpath='{.data.tls\.key}' | base64 -d > "$MTLS_DIR/tls.key"

openshell gateway remove openshift 2>/dev/null || true
openshell gateway add "https://${ROUTE_HOST}:443" --local --name openshift
openshell status
```

You should see `Status: Connected` and `Authentication: Authenticated (mTLS transport)`.

> **Note:** `--local` tells the CLI to use the mTLS certs from the gateway
> config directory. Despite the name, it works for any endpoint where you
> manage the client certs yourself — not just Docker-local gateways.

### 4. Existing sandboxes

If you had sandboxes created before regenerating certs, they'll be stuck in
`Provisioning` because their JWT tokens were signed with the old keys.
Delete and recreate them.

## Verify: hello-world sandbox

This mirrors NVIDIA's own quickstart pattern — create a sandbox, confirm an
outbound call is blocked by the default policy, apply a policy that allows it,
confirm it now succeeds. No credentials or providers involved; this only
proves sandbox creation, network isolation, and policy hot-reload all work on
this cluster.

### 1. Create a sandbox

```bash
openshell sandbox create --name hello-world -- bash
```

### 2. Confirm outbound calls are blocked

```bash
openshell sandbox exec -n hello-world -- curl -sS https://api.github.com/zen
```

This should fail — the default policy blocks all outbound traffic.

### 3. Allow a specific endpoint

```bash
openshell policy update hello-world \
  --add-endpoint api.github.com:443:read-only:rest:enforce \
  --binary /usr/bin/curl \
  --wait
```

### 4. Confirm the call now succeeds

```bash
openshell sandbox exec -n hello-world -- curl -sS https://api.github.com/zen
```

You should see a response (a random GitHub zen quote).

### 5. Clean up

```bash
openshell sandbox delete hello-world
```

## Smoke test: provider credential injection

Once the hello-world test passes, this optional step proves that providers
can inject credentials into sandbox outbound calls. Bring any
OpenAI-compatible API — you need a base URL and an API key.

### 1. Set your LLM provider details

```bash
export OPENAI_API_KEY="<your-key>"
export OPENAI_BASE_URL="https://<your-provider>/v1"   # e.g. https://api.openai.com/v1
export OPENAI_MODEL="<model-name>"                     # e.g. gpt-4o
```

Extract the hostname for the network policy:

```bash
LLM_HOST=$(echo "$OPENAI_BASE_URL" | sed 's|https\?://||;s|/.*||')
```

### 2. Import the provider profile and create a provider

```bash
openshell provider profile import --file providers/openai-profile.yaml
openshell provider create --name byo-openai --type openai \
  --credential OPENAI_API_KEY \
  --config "base_url=$OPENAI_BASE_URL"
```

### 3. Attach it to the sandbox and allow the endpoint

```bash
openshell sandbox provider attach hello-world byo-openai
openshell policy update hello-world \
  --add-endpoint "${LLM_HOST}:443:read-write:rest:enforce" \
  --binary /usr/bin/curl --wait
```

### 4. Call the API from inside the sandbox

```bash
openshell sandbox exec -n hello-world -- \
  curl -sS "${OPENAI_BASE_URL}/chat/completions" \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $OPENAI_API_KEY" \
    -d '{"model":"'"$OPENAI_MODEL"'","messages":[{"role":"user","content":"Say hello in one sentence."}],"max_tokens":50}'
```

You should get a JSON response with a chat completion.

> **How credential injection works:** the provider sets `OPENAI_API_KEY`
> inside the sandbox to a *resolve placeholder*
> (`openshell:resolve:env:v168...`), not the real key. When curl puts that
> placeholder in the `Authorization: Bearer` header, the gateway proxy
> intercepts it and swaps in the real API key before forwarding to the LLM.
> The actual secret never enters the sandbox. Application code works exactly
> as it would outside the sandbox — `Authorization: Bearer $OPENAI_API_KEY`
> — but the key stays on the gateway side.

### 5. Clean up

```bash
openshell sandbox delete hello-world
openshell provider delete byo-openai
```

## Definition of done

- [ ] Agent Sandbox controller running in `agent-sandbox-system`
- [ ] Namespace created, `privileged` SCC granted to the `openshell-sandbox` service account
- [ ] `helm install` succeeds, `statefulset/openshell` reports ready
- [ ] Gateway reachable via `oc port-forward` or passthrough Route, `openshell status` succeeds
- [ ] Hello-world sandbox created
- [ ] Outbound call blocked by the default policy
- [ ] Policy update applied, the same call now succeeds
- [ ] (Optional) Provider credential injection smoke test passes
- [ ] Sandbox deleted, namespace left clean for the next demo

## Troubleshooting

| Symptom | Likely cause |
|---|---|
| Sandbox pods stuck `Pending` / SCC admission errors | `privileged` SCC not granted to `openshell-sandbox` service account in `$OPENSHELL_NAMESPACE`. Note: the SA is `openshell-sandbox`, **not** `default` |
| `helm install` rejects pod security fields | `podSecurityContext.fsGroup` / `securityContext.runAsUser` not nulled out — OpenShift's admission controller needs to assign these itself |
| Gateway pod stuck in `ContainerCreating`, event says `secret "openshell-jwt-keys" not found` | Do **not** set `pkiInitJob.enabled: false`. The PKI init job generates the sandbox JWT signing keys even when TLS is disabled. Leave it at the default (`true`) |
| `helm install` says chart `not found` at `oci://ghcr.io/nvidia/openshell/helm-chart` | The chart version must **not** have a `v` prefix. Use `0.0.97`, not `v0.0.97`. The Git tag uses `v0.0.97` but the OCI chart is published as `0.0.97` |
| `kubectl apply` for Agent Sandbox returns 404 | The manifest file is `sandbox.yaml`, not `manifest.yaml` — see [Installing Agent Sandbox](#installing-agent-sandbox) |
| Scripts fail with `: OPENSHELL_NAMESPACE: set in .env` | Variables are sourced but not exported. Use `export $(grep -v '^#' .env | xargs)` and `export $(grep -v '^#' ../../.env | xargs)` instead of plain `source` |
| Sandbox pods never schedule at all | Agent Sandbox controller/CRDs not installed before the chart |
| Outbound call still blocked after adding an endpoint to the policy | The policy enforces a **binary allowlist**. Adding an endpoint alone is not enough — you must also specify which binary is allowed to use it: `openshell policy update <sandbox> --add-endpoint host:port:access:proto:enforce --binary /usr/bin/curl`. Use `readlink -f <binary>` inside the sandbox to find the canonical path if symlinks are involved |
| `openshell status` shows Connected but `Authentication: Failed (missing authorization header)` | The gateway requires a gRPC authorization header even with mTLS. Set `server.auth.allowUnauthenticatedUsers=true` via `--set` at `helm install`/`upgrade` time, or configure OIDC. This is required for the passthrough Route path |
| Sandboxes stuck in `Provisioning` after regenerating TLS secrets | The sandbox JWT tokens were signed with the old keys. Delete and recreate the sandboxes |

## Using the Vagrant VM (macOS Intel only)

The repo includes a [Vagrantfile](../../Vagrantfile) that creates a Fedora VM
with `oc`, `helm`, `kubectl`, `openshell`, and bash completions pre-installed.

> **This Vagrantfile is specific to macOS on Intel (x86_64)** using QEMU via
> [vagrant-qemu](https://github.com/ppggff/vagrant-qemu). It will **not**
> work on Apple Silicon without changes — the Vagrant box and CLI binaries
> are all x86_64. For other host platforms, adapt the box, provider, and
> download URLs, or just install the tools directly on your machine.

```bash
# One-time setup
brew install qemu
vagrant plugin install vagrant-qemu

# Start the VM
vagrant up

# SSH in
vagrant ssh

# The repo is synced to /vagrant (excluding .git, .env, .vagrant)
cd /vagrant/demos/base
```

If you prefer to run commands from outside the VM, set up a shell alias:

```bash
alias vsh='vagrant ssh -c'

# Then use it like:
vsh "openshell status"
vsh "openshell sandbox list"
```

To copy your `.env` into the VM (it's excluded from rsync for safety):

```bash
vagrant upload .env /vagrant/.env
```

After editing files locally, sync them into the VM:

```bash
vagrant rsync
```

## Next steps

Once the above is all green, go to a demo — start with
[`demos/keycloak-oidc/README.md`](../keycloak-oidc/README.md).
