# How sandbox network isolation actually works

[Scene 4](../README.md#scene-4--bob-overreaches) shows the *application-level*
boundary: a shared MCP server's own `assert_owns_client` check rejects a
request for a client outside the caller's book. This document covers the
*network-level* boundary underneath it — what a compromised or malicious
agent process inside a sandbox could reach if it tried to open a raw socket
toward another banker's sandbox directly, with no MCP server, no Envoy, and
no JWT check anywhere in the path.

## The mechanism

The sandbox pod's outer container (`agent`, PID 1 — the OpenShell sandbox
supervisor) sits on a completely normal pod network: a real pod IP, full
routes to the pod and service CIDRs, the works. But the command environment
`sandbox exec` actually runs commands in — where `curl` or `claude`
execute — is a **separate, nested network namespace**, connected to that
outer container by exactly one veth pair. Its entire routing table:

```
$ openshell sandbox exec -n demo-bob --workspace bob -- ip route
default via 10.200.0.1 dev veth-s-b58ba810
10.200.0.0/24 dev veth-s-b58ba810 proto kernel scope link src 10.200.0.2
```

That's it — one default route, to the supervisor's own local proxy, and
nothing else. There is no interface, and therefore no route, to the pod
CIDR (`10.128.0.0/14`), the service CIDR (`172.30.0.0/16`), or any other
pod's IP, including another banker's sandbox.

Two consequences follow from that routing table:

1. **Proxy-aware traffic** (`curl`, which honors the sandbox's
   `ALL_PROXY=http://10.200.0.1:3128`) reaches the supervisor's own
   enforcing proxy — the same proxy where the `openshell policy update
   --add-endpoint --binary` allowlist from
   [step 5's setup](../README.md#5-run-the-demo) is applied. A request to
   an allowlisted MCP server succeeds; a request to anything else (e.g.
   another banker's sandbox pod IP) gets an explicit **403 Forbidden** from
   the proxy itself.
2. **Traffic that bypasses the proxy** (a raw `/dev/tcp` connection, `nc`,
   `curl --noproxy '*'`) fails instantly with **`Connection refused`**.

A Kubernetes `NetworkPolicy` (`openshell-sandbox-ssh`) additionally
restricts inbound TCP:2222 on sandbox pods to only the OpenShell gateway —
a real, separate control protecting the gateway's own exec/control-plane
channel — but it isn't what's doing the work described above; the netns
routing table alone is sufficient to explain every observed result.

## What this means

Sandbox network isolation is enforced at the process/namespace level,
independent of any `NetworkPolicy`, Envoy check, or MCP server logic. Even
a fully compromised agent process running inside one banker's sandbox has
exactly one way to reach anything outside its own namespace — the
supervisor's own proxy, itself gated by that sandbox's binary/endpoint
allowlist. It has no raw network path to another banker's sandbox to even
attempt to exploit, regardless of what identity or token it holds, and
regardless of what that other sandbox happens to be running.

This is a **third, independent isolation layer**, underneath the two
covered elsewhere in the guide: (1) MCP-server tenant-ownership
(`assert_owns_client`,
[Scene 4](../README.md#scene-4--bob-overreaches)), (2) OpenShell workspace
membership (CLI-level, see
[Workspace isolation](../README.md#workspace-isolation) and the
concurrent-XDG-identity proof in
[How to follow this guide](../README.md#how-to-follow-this-guide)).

## If you want to test this yourself

Don't probe an arbitrary port on another sandbox's pod IP and treat
`Connection refused` alone as proof of isolation — that error is also
exactly what you'd see if the network path exists fine but nothing happens
to be listening on that port (a probe against an empty port can't
distinguish "isolated" from "nobody's home"). To test the real boundary,
dial a service you already know is alive — e.g. `mcp-portfolio`'s real
ClusterIP on port 8000, which normally works fine *through* the proxy —
directly, bypassing the proxy (`curl --noproxy '*'`, `nc`, or raw
`/dev/tcp`), from inside a sandbox. If that fails identically to a probe
against an unrelated pod, the namespace boundary — not the destination's
availability — is what's actually being tested.
