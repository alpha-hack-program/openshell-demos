# base — OpenShell on OpenShift, generic install + hello world

Demo-agnostic. Installs the OpenShell gateway on OpenShift and proves the
install works with a minimal sandbox test. Other demos layered on top via a Helm
values *overlay* in each `demos/<name>/helm/values-overlay.yaml`, never by
editing anything in this folder.

## Prerequisites

| Tool / access | Notes |
|---|---|
| `oc` | Logged into the target cluster, with rights to grant SCCs |
| `helm` 3.x | |
| `kubectl` | Compatible with cluster version |
| `openshell` CLI | See [Installing the CLI](#installing-the-cli) below |
| OpenShift 4.x cluster | |
| Agent Sandbox controller + CRDs | See [Installing Agent Sandbox](#installing-agent-sandbox) below — must be done **before** `helm install` |

You can run these tools from any machine that can reach the cluster: your
laptop, a jump host, a VM, a container. The repo includes a
[Vagrantfile](../Vagrantfile) that provisions a Fedora VM with everything
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
The `spire-spiffe-keycloak` demo adds Keycloak as an OIDC provider, enabling
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

## Install

The scripts expect environment variables to be **exported**, not just sourced.
Use the `export` pattern below, or add `set -a` / `set +a` around the source:

```bash
export $(grep -v '^#' ../.env | xargs)

./scripts/00-prereqs-check.sh
./scripts/01-namespace-and-scc.sh
./scripts/02-install-openshell.sh
./scripts/03-connect-gateway.sh
```

## Exposing the gateway via passthrough Route

By default the install scripts use `oc port-forward` to reach the gateway on
`localhost:8080`. This is fine for single-user evaluation but requires an
active terminal. A **passthrough Route** lets you (and others) reach the
gateway over the network without a port-forward.

As explained in [How OpenShell networking works](#how-openshell-networking-works),
gRPC requires HTTP/2 end-to-end, so only a passthrough Route works — it
forwards raw TLS to the pod, which terminates the connection itself.

### 1. Re-deploy with the route hostname in the server cert

The route hostname must be in the server certificate's SANs. Pass it as a
`--set` override — don't hardcode it in `values-openshift.yaml`:

```bash
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"

# If this is a fresh install, just add the --set to your helm install.
# If upgrading an existing install, delete the TLS secrets first so the
# PKI job regenerates them with the new SAN:
oc -n "$OPENSHELL_NAMESPACE" delete secret \
  openshell-server-tls openshell-client-tls openshell-jwt-keys

helm upgrade --install openshell oci://ghcr.io/nvidia/openshell/helm-chart \
  --version "$OPENSHELL_CHART_VERSION" -n "$OPENSHELL_NAMESPACE" \
  -f base/helm/values-openshift.yaml \
  --set "pkiInitJob.serverDnsNames[0]=${ROUTE_HOST}" \
  --set "server.auth.allowUnauthenticatedUsers=true"
```

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

```bash
./scripts/04-hello-world-sandbox.sh
```

This mirrors NVIDIA's own quickstart pattern — create a sandbox, confirm an
outbound call is blocked by the default policy, apply a policy that allows it,
confirm it now succeeds. No credentials or providers involved; this only
proves sandbox creation, network isolation, and policy hot-reload all work on
this cluster.

## Smoke test: provider credential injection

Once the hello-world test passes, this optional step proves that providers
can inject credentials into sandbox outbound calls. Uses DeepSeek (or any
OpenAI-compatible API) as the target.

### 1. Import an OpenAI-compatible provider profile

```bash
openshell provider profile import --file providers/openai-profile.yaml
```

### 2. Create a provider with your API key

```bash
export OPENAI_API_KEY=<your-key>
openshell provider create --name deepseek --type openai \
  --credential OPENAI_API_KEY \
  --config base_url=https://api.deepseek.com
```

### 3. Attach it to the sandbox and allow the endpoint

```bash
openshell sandbox provider attach hello-world deepseek
openshell policy update hello-world \
  --add-endpoint api.deepseek.com:443:read-write:rest:enforce \
  --binary /usr/bin/curl --wait
```

### 4. Call the API from inside the sandbox

```bash
openshell sandbox exec -n hello-world -- \
  curl -sS https://api.deepseek.com/v1/chat/completions \
    -H "Content-Type: application/json" \
    -H "Authorization: Bearer $OPENAI_API_KEY" \
    -d '{"model":"deepseek-chat","messages":[{"role":"user","content":"Say hello in one sentence."}],"max_tokens":50}'
```

You should get a JSON response with a chat completion.

> **How credential injection works:** the provider sets `OPENAI_API_KEY`
> inside the sandbox to a *resolve placeholder*
> (`openshell:resolve:env:v168...`), not the real key. When curl puts that
> placeholder in the `Authorization: Bearer` header, the gateway proxy
> intercepts it and swaps in the real API key before forwarding to DeepSeek.
> The actual secret never enters the sandbox. Application code works exactly
> as it would outside the sandbox — `Authorization: Bearer $OPENAI_API_KEY`
> — but the key stays on the gateway side.

### 5. Clean up

```bash
openshell sandbox delete hello-world
openshell provider delete deepseek
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
| Scripts fail with `: OPENSHELL_NAMESPACE: set in .env` | Variables are sourced but not exported. Use `export $(grep -v '^#' ../.env | xargs)` instead of `source ../.env` |
| Sandbox pods never schedule at all | Agent Sandbox controller/CRDs not installed before the chart |
| Outbound call still blocked after adding an endpoint to the policy | The policy enforces a **binary allowlist**. Adding an endpoint alone is not enough — you must also specify which binary is allowed to use it: `openshell policy update <sandbox> --add-endpoint host:port:access:proto:enforce --binary /usr/bin/curl`. Use `readlink -f <binary>` inside the sandbox to find the canonical path if symlinks are involved |
| `openshell status` shows Connected but `Authentication: Failed (missing authorization header)` | The gateway requires a gRPC authorization header even with mTLS. Set `server.auth.allowUnauthenticatedUsers=true` via `--set` at `helm install`/`upgrade` time, or configure OIDC. This is required for the passthrough Route path |
| Sandboxes stuck in `Provisioning` after regenerating TLS secrets | The sandbox JWT tokens were signed with the old keys. Delete and recreate the sandboxes |

## Using the Vagrant VM (macOS Intel only)

The repo includes a [Vagrantfile](../Vagrantfile) that creates a Fedora VM
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
cd /vagrant/base
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
[`demos/spire-spiffe-keycloak/README.md`](../demos/spire-spiffe-keycloak/README.md).
