# Sandbox service patterns

Reusable patterns for building, deploying, and exposing long-running
services inside OpenShell sandboxes on remote (OpenShift) gateways.
Distilled from the agent-proxy lifecycle validation (2026-08-18, OpenShell
0.0.106).

## 1. Custom sandbox images

### Static binaries required

Sandbox base images ship older glibc versions than current Fedora/RHEL
workstations. Binaries compiled on the host will fail with
`GLIBC_x.xx not found` unless statically linked.

For Rust projects, use the musl target:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build --release --target x86_64-unknown-linux-musl
```

### Containerfile layout

Each custom image gets its own directory under `demos/<name>/images/<variant>/`.
The Containerfile layers the service binary onto the agent's base image.
A `.gitignore` excludes the copied binary (it's a build artifact, not
source).

```
demos/<name>/images/<variant>/
├── Containerfile
├── .gitignore      # agent-proxy
└── agent-proxy     # copied here before build, gitignored
```

The binary must be copied into the image directory before building — the
build context is the directory itself, so relative paths outside it
(e.g. `../../util/`) won't resolve:

```bash
cp util/agent-proxy/target/x86_64-unknown-linux-musl/release/agent-proxy \
   demos/<name>/images/<variant>/
```

### Building and pushing (remote gateways)

The OpenShell CLI's `--from <directory>` mode builds images locally and
only works for **local gateways**. On remote gateways (OpenShift), it
errors with:

> local Dockerfile sources are only supported for local gateways

Build and push manually, then pass the image reference to `--from`:

```bash
REGISTRY="${GARAK_IMAGE_REGISTRY:-quay.io/atarazana}"
IMAGE="${REGISTRY}/<variant>:latest"

podman build -t "$IMAGE" \
    -f demos/<name>/images/<variant>/Containerfile \
    demos/<name>/images/<variant>/
podman push "$IMAGE"

openshell sandbox create --name my-sandbox --from "$IMAGE" -- true
```

**Toolbox note:** if you're working inside a Fedora toolbox, `podman`
fails with a user namespace mismatch. Use `flatpak-spawn --host podman`
to run podman on the host:

```bash
flatpak-spawn --host podman build -t "$IMAGE" ...
flatpak-spawn --host podman push "$IMAGE"
```

### Containerfile naming

The files are named `Containerfile` (Podman-idiomatic). The OpenShell
CLI's directory mode only looks for `Dockerfile`, but since that mode
doesn't work on remote gateways anyway, the name doesn't matter — we
always build with `podman build -f`.

### Upload approach (no custom image)

For ad-hoc runs or local testing, skip the custom image entirely. Use the
stock agent base image and upload the binary at runtime:

```bash
# Create sandbox from stock image
openshell sandbox create --name my-sandbox \
    --from quay.io/aipcc/base-images/agentic/codex:0.0.1-1786355012 -- true

# Upload the static binary
openshell sandbox upload -n my-sandbox \
    path/to/my-service /usr/local/bin/my-service

# Make executable, start, expose
openshell sandbox exec -n my-sandbox -- \
    bash -c 'chmod +x /usr/local/bin/my-service && nohup /usr/local/bin/my-service --port 8080 > /sandbox/service.log 2>&1 &'
openshell service expose my-sandbox 8080
```

No Containerfile, no registry credentials, no image build. Best for:
- Ad-hoc testing and demos
- Rapid iteration (upload a new binary, restart)
- Environments without registry access

The custom image approach (above) is better when:
- The binary must always be present at sandbox creation (CI pipelines)
- Reproducibility matters (image tag pins a specific version)
- Multiple users create sandboxes from the same image

## 2. Running services inside sandboxes

### Background startup

Start a long-running process inside a sandbox with `sandbox exec` and
`nohup`:

```bash
openshell sandbox exec -n <sandbox> -- \
    bash -c 'nohup /usr/local/bin/my-service --port 8080 > /sandbox/service.log 2>&1 &'
```

The `bash -c '... &'` wrapper is required — `sandbox exec` waits for
the command to exit, and without `&` it would block. Wrapping in
`bash -c` ensures the backgrounded process is detached and `exec`
returns immediately.

### Stopping a service

```bash
openshell sandbox exec -n <sandbox> -- bash -c 'pkill -f my-service 2>/dev/null; true'
```

The `; true` prevents a non-zero exit if the process isn't running.

### Environment variables

Pass configuration via environment variables at `exec` time or
set them in the `nohup` invocation:

```bash
openshell sandbox exec -n <sandbox> -- \
    bash -c 'MY_VAR=value nohup my-service &'
```

For secrets, use providers (`--provider`) — never pass secrets via
`--env` or inline in `bash -c`.

## 3. Exposing services

### `service expose` (gateway-managed URL)

Creates an HTTPS endpoint routed through the gateway. Use this when
the service needs to be reachable by other workloads on the cluster
(e.g. a Garak K8s Job, a Prometheus scraper):

```bash
openshell service expose <sandbox> <port>
# => URL: https://default--<sandbox>.openshell.localhost/
```

The URL uses **Host-header routing** through the gateway's OpenShift
Route. External clients reach the service via:

```bash
ROUTE_HOST="openshell-${OPENSHELL_NAMESPACE}.${CLUSTER_APPS_DOMAIN}"
curl -sk "https://${ROUTE_HOST}/path" \
    -H "Host: default--<sandbox>.openshell.localhost"
```

On-cluster workloads (K8s Jobs, other pods) use the same pattern. The
gateway Route is the single entry point — no direct pod-to-pod
connectivity needed.

### `--forward` (local SSH tunnel)

Creates a local-only port forward via SSH. Use this for interactive
debugging from your workstation — not suitable for cluster workloads
(Garak, Prometheus, etc.) since the tunnel terminates on your machine:

```bash
openshell sandbox create --name my-sandbox --forward 8080 ...
# => localhost:8080 tunnels to sandbox:8080
```

### Listing and cleaning up

```bash
openshell service list                    # all exposed services
openshell service list <sandbox>          # for a specific sandbox
openshell service delete <sandbox>        # remove the exposed endpoint
```

## 4. Use cases

These patterns apply to any long-running service inside a sandbox:

| Use case | Service | Port | Who connects |
|---|---|---|---|
| Red-team evaluation | `agent-proxy` (OpenAI-compatible) | 8080 | Garak K8s Job via EvalHub |
| Metrics collection | Prometheus exporter | 9090 | Prometheus scraper on cluster |
| API testing | Test server / mock | any | CI jobs, other sandboxes |
| Development | Dev server / debugger | any | Developer workstation (use `--forward`) |

For all cluster-facing use cases, prefer `service expose` over `--forward`.
