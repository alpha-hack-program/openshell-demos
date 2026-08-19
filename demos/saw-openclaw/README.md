# saw-openclaw — OpenClaw on SAW (Secure Agent Workspace)

Deploys [Secure Agent Workspace](https://github.com/validatedpatterns-sandbox/secure-agent-workspace/)
on bare-metal OpenShift and runs OpenClaw inside a SAW sandbox with Google
Gemini inference. Includes workarounds for known deployment issues in the
current SAW release.

> **This demo does not use the standard OpenShell Helm chart.** SAW is a
> separate deployment pattern that wraps OpenShell inside KubeVirt VMs with
> additional isolation layers (egress proxy, governance interceptor). The
> other demos in this repo deploy OpenShell directly via
> `oci://ghcr.io/nvidia/openshell/helm-chart` — this one deploys the
> [SAW Helm chart](https://github.com/validatedpatterns-sandbox/secure-agent-workspace/)
> instead.

## Purpose

Show how SAW provides VM-level sandbox isolation on OpenShift using
KubeVirt, and prove that OpenClaw can run inference inside that sandbox.
The demo also documents workarounds for issues in the current SAW release
that prevent the automated onboard from completing.

## Prerequisites

| Tool / access | Notes |
|---|---|
| `oc` | Logged into the target cluster with cluster-admin rights |
| `helm` 3.x | |
| `virtctl` | For SSH into the SAW VM — download from the cluster's `ConsoleCLIDownload` resource |
| `make` | SAW uses Makefile targets for deployment |
| `curl`, `python3` | For OIDC token exchange and config generation |
| Bare-metal OpenShift 4.22+ | ROSA and other managed platforms do not support KubeVirt |
| Google Gemini API key | From [AI Studio](https://aistudio.google.com/apikey) |

### Why bare-metal?

SAW uses OpenShift Virtualization (KubeVirt) to run per-user VMs that host
the OpenShell gateway and sandbox containers. KubeVirt requires hardware
virtualization support (nested virt), which managed platforms like ROSA do
not provide. The simplest way to get a compatible cluster is
[demo.redhat.com](https://catalog.demo.redhat.com/catalog/all?search=Red+Hat+OpenShift+Container+Platform+Cluster):

1. Search "Red Hat OpenShift Container Platform Cluster (Multi-Cloud)"
2. Order > Practice / Enablement > Trying out technical solutions
3. Settings: OCP 4.22, Multi-node, 2 workers, 8 CPU, 32 GB memory
4. Cluster auto-deletes after 48 hours, hibernates every 6 hours

### Installing virtctl

Download from the cluster itself to ensure version compatibility:

```bash
VIRTCTL_URL=$(oc get ConsoleCLIDownload virtctl-clidownloads-kubevirt-hyperconverged \
  -o jsonpath='{.spec.links[?(@.text=="Download virtctl for Linux for x86_64")].href}')
curl -fsSL "$VIRTCTL_URL" | tar xz -C /usr/local/bin virtctl
virtctl version --client --short
```

On macOS, replace the `text` filter with the Darwin variant, or download
the binary from the [KubeVirt releases](https://github.com/kubevirt/kubevirt/releases)
matching the cluster's KubeVirt version.

## What this demo deploys

SAW deploys a different stack than the standard OpenShell chart:

| Component | Where | Purpose |
|---|---|---|
| KubeVirt VM | `openshell-agents` namespace | Per-user isolated host for gateway + sandboxes |
| OpenShell gateway | Inside the VM (systemd service) | Manages sandboxes, policy enforcement, credential injection |
| OpenShell supervisor | Inside the VM | Runs sandbox containers with policy proxy |
| Sandbox container | Docker inside the VM | Isolated environment where OpenClaw runs |
| Keycloak (RHBK) | `openshell-agents` namespace | OIDC identity provider |
| Golden image | Internal registry | Pre-built VM disk with OS and dependencies |

### How SAW differs from the base demo

```
Base demo (demos/base/):
  OpenShift Pod → OpenShell gateway → Sandbox container

SAW demo (demos/saw-openclaw/):
  OpenShift Pod → KubeVirt VM → OpenShell gateway → Sandbox container
                                      ↓
                               Egress proxy (controls outbound traffic)
                               Governance interceptor (optional policy layer)
```

The VM layer provides stronger isolation — the sandbox runs inside a
container, inside a VM, inside a pod. The egress proxy controls all
outbound network access from the sandbox, routing traffic through the
gateway's policy proxy.

## Steps

### 1. Check prerequisites

```bash
cd demos/saw-openclaw
cp .env.example .env
# Edit .env with your cluster and API key details

export $(grep -v '^#' .env | xargs)
export $(grep -v '^#' ../../.env | xargs)

./scripts/00-prereqs-check.sh
```

### 2. Clone and configure SAW

```bash
git clone "$SAW_REPO" /tmp/saw
SAW_DIR=/tmp/saw

cd "$SAW_DIR"
cp values-secret.yaml.template ~/values-secret.yaml
```

Edit `~/values-secret.yaml`:

```yaml
- name: inference
  fields:
  - name: provider
    value: gemini
  - name: model
    value: gemini-3.6-flash
  - name: api_key
    value: <your-gemini-api-key>
```

Generate SSH keys for VM access:

```bash
mkdir -p ~/.generated-ssh-keys
ssh-keygen -t ed25519 -f ~/.generated-ssh-keys/sandbox-ssh -N "" -C "saw-demo"
```

Update your `.env` with `SAW_SSH_KEY_PATH=~/.generated-ssh-keys/sandbox-ssh`.

### 3. Deploy SAW

```bash
cd "$SAW_DIR"

# Log into the cluster
oc login --server="$OCP_API" --username="$OCP_USER" --password="$OCP_PASSWORD"

# Run the SAW quickstart targets
make check-prereqs
make copy-images
make keycloak-issuer

# Create the sandbox VM — governance must be disabled (see known issues)
OWNER=alice GOVERNANCE_ENABLED=false \
  PROVIDER=gemini MODEL=gemini-3.6-flash API_KEY="$GEMINI_API_KEY" \
  make openshell-saw-create
```

Monitor the setup job:

```bash
oc logs -n openshell-agents -f job/$(oc get jobs -n openshell-agents \
  -o jsonpath='{.items[-1].metadata.name}')
```

The setup job will install gateway/supervisor binaries and attempt
`nemoclaw onboard`. **The onboard step will fail** — this is expected with
the current SAW release. See [Known issues](#known-issues) for details.

### 4. Verify the VM is running

```bash
oc get vmi -n openshell-agents
# NAME           AGE   PHASE     IP           NODENAME   READY
# mschimun-test  10m   Running   10.232.0.x   ...        True
```

Confirm SSH access (the VM user is `cloud-user`, not `fedora`):

```bash
virtctl -n openshell-agents ssh \
  --identity-file="$SAW_SSH_KEY_PATH" \
  cloud-user@vm/mschimun-test \
  --local-ssh-opts="-oStrictHostKeyChecking=no" \
  --local-ssh-opts="-oUserKnownHostsFile=/dev/null" \
  --command="systemctl --user is-active openshell-gateway.service"
# active
```

If the SSH key from your local machine doesn't work, extract it from the
cluster secret:

```bash
oc get secret openshell-aap-ssh -n openshell-agents \
  -o jsonpath='{.data.key}' | base64 -d > /tmp/saw-ssh-key
chmod 600 /tmp/saw-ssh-key
SAW_SSH_KEY_PATH=/tmp/saw-ssh-key
```

### 5. Apply the inference workaround

The `nemoclaw onboard` failure leaves the sandbox without a configured
inference provider. This step replicates what onboard would have done:

```bash
cd demos/saw-openclaw
./scripts/02-workaround-inference.sh
```

The script does four things:

1. **Gets an OIDC token** from Keycloak via a direct password grant
   (ROPC) using the `alice/alice` test credentials
2. **Grants workspace access** to the mTLS client — without this,
   all provider and sandbox operations fail with permission errors
3. **Updates the sandbox network policy** to allow outbound connections
   to `generativelanguage.googleapis.com` for the `node` and `curl`
   binaries
4. **Configures OpenClaw** inside the sandbox to use the Google
   Generative AI API with `gemini-3.6-flash`, including an explicit
   model definition (the model is not in OpenClaw's built-in registry)

### 6. Verify OpenClaw inference

```bash
./scripts/03-verify-openclaw.sh
```

Or test manually:

```bash
# SSH into the VM
virtctl -n openshell-agents ssh \
  --identity-file="$SAW_SSH_KEY_PATH" \
  cloud-user@vm/mschimun-test \
  --local-ssh-opts="-oStrictHostKeyChecking=no" \
  --local-ssh-opts="-oUserKnownHostsFile=/dev/null"

# Inside the VM, run OpenClaw inference
openshell sandbox exec -n openclaw-test --no-tty -- \
  openclaw infer model run --model google/gemini-3.6-flash \
  --prompt "What is the capital of France? Reply in one word." --no-color
# Expected output: Paris
```

## Configuration reference

| Variable | Source | Description |
|---|---|---|
| `OPENSHELL_NAMESPACE` | demo `.env` | Kubernetes namespace for all SAW resources (default: `openshell-agents`) |
| `SAW_REPO` | demo `.env` | Git URL for the SAW repository |
| `SAW_DIR` | demo `.env` | Local path to the cloned SAW repo |
| `OCP_API` | demo `.env` | OpenShift API server URL |
| `OCP_USER` / `OCP_PASSWORD` | demo `.env` | Cluster admin credentials |
| `SAW_SANDBOX_NAME` | demo `.env` | Name for the sandbox container inside the VM |
| `SAW_SANDBOX_OWNER` | demo `.env` | Keycloak user that owns the sandbox (default: `alice`) |
| `SAW_SSH_KEY_PATH` | demo `.env` | Path to the SSH private key for VM access |
| `GEMINI_API_KEY` | demo `.env` | Google Gemini API key |
| `GEMINI_MODEL` | demo `.env` | Gemini model name (default: `gemini-3.6-flash`) |

## Secrets and security notes

- **Gemini API key** is written directly into the OpenClaw config inside
  the sandbox. In production, use the OpenShell provider credential
  injection mechanism (via `openshell provider create`) so the key never
  enters the sandbox boundary. This demo bypasses that because the
  `google-vertex-ai` provider profile does not match the direct Gemini
  API format.
- **Keycloak test credentials** (`alice/alice`) are hardcoded in SAW's
  realm export. Never use these in production.
- **SSH keys** are generated locally and injected via cloud-init. The
  private key is stored in a Kubernetes secret (`openshell-aap-ssh`).
- The egress proxy inside the sandbox **blocks all outbound traffic by
  default**. The workaround script explicitly allowlists the Gemini API
  endpoint. This is a security feature — any endpoint the sandbox can
  reach must be explicitly approved via `openshell policy update`.

## Known issues

### `openshell==0.0.99+rhaiv.0` not on PyPI

The setup job tries to `pip install openshell==0.0.99+rhaiv.0`, but
this version does not exist on PyPI (available version is `0.0.99`
without the `+rhaiv.0` suffix). This causes the `nemoclaw onboard`
step to fail, preventing:

- Inference provider configuration on the gateway
- Proxy allowlist configuration
- Sandbox network policy setup

**Workaround:** step 5 above replicates the onboard steps manually.

### Governance interceptor not deployed

The default `GOVERNANCE_ENABLED=true` causes the gateway to crash-loop
because it tries to connect to
`governance-interceptor.openshell-agents.svc.cluster.local:18081` with
a `fail_closed` policy, but the governance interceptor chart is not
deployed.

**Workaround:** set `GOVERNANCE_ENABLED=false` when creating the sandbox.

### `gemini-2.5-flash` deprecated

Google deprecated the `gemini-2.5-flash` model. API calls return 404
with a suggestion to use newer models. The SAW quickstart default
should be updated.

**Workaround:** use `gemini-3.6-flash` instead.

### No built-in `gemini` provider profile

OpenShell's provider system has a `google-vertex-ai` profile (for
service-account-based Vertex AI access) but no `gemini` profile for
direct API key authentication. The SAW `setup-nemoclaw.sh` fallback
references a `gemini` provider type that doesn't exist in the gateway's
built-in profile list.

**Workaround:** configure the Gemini API key directly in OpenClaw's
config files rather than through the gateway's provider system.

### VM user mismatch

The SAW golden image uses `cloud-user` as the SSH user, but some SAW
scripts reference `fedora`. Use `cloud-user` when SSH'ing via `virtctl`.

### TLS auto-connect error

When creating a sandbox with `openshell sandbox create`, the
auto-connect step may fail with `invalid peer certificate:
UnknownIssuer`. The sandbox itself is created and functional — the
error only affects the initial auto-attach.

## Definition of done

- [ ] Bare-metal OpenShift 4.22+ cluster provisioned
- [ ] OpenShift Virtualization operator installed
- [ ] SAW repo cloned and configured
- [ ] `make openshell-saw-create` completes (setup job finishes)
- [ ] VM running and accessible via `virtctl ssh`
- [ ] Gateway service active inside the VM
- [ ] Inference workaround applied
- [ ] Sandbox network policy allows `generativelanguage.googleapis.com`
- [ ] OpenClaw config points to `google/gemini-3.6-flash`
- [ ] `openclaw infer model run` returns a valid response from inside the sandbox
- [ ] Code generation test passes
- [ ] Reasoning test passes

## Open risks / things to verify

- **[VERIFY]** The `GOVERNANCE_ENABLED=false` workaround disables an
  entire security layer. Verify whether the governance interceptor chart
  is meant to be deployed separately or bundled with SAW.
- **[VERIFY]** The `cloud-user` vs `fedora` SSH user mismatch may be
  specific to the golden image version used. Confirm with the SAW team
  whether this is intentional.
- **[VERIFY]** Once the `openshell` CLI version mismatch is fixed
  upstream, the workaround script (step 5) should no longer be needed.
  Test a clean deploy without it after the fix lands.
- **[VERIFY]** The provider credential injection path for Google Gemini
  (direct API key, not Vertex AI service account) is not supported by
  any built-in provider profile. Confirm whether a `gemini` profile is
  planned.

## References

- SAW repo: https://github.com/validatedpatterns-sandbox/secure-agent-workspace/
- OpenShell docs: https://docs.nvidia.com/openshell
- OpenShift Virtualization docs: https://docs.openshift.com/container-platform/latest/virt/about_virt/about-virt.html
- Google Gemini API: https://ai.google.dev/gemini-api
- demo.redhat.com: https://catalog.demo.redhat.com
