# mcp-kyc-compliance

Rust MCP server exposing simplified, fictional KYC/compliance checks and
regulatory-guidance search for a banker's own book of clients. Runs behind
an Envoy sidecar that validates the JWT signature against Keycloak before
the request reaches here — this binary does **not** validate a signature,
it only decodes the token payload to extract the caller's identity
(`preferred_username` → `banker_id`, `realm_access.roles` → `roles`). If
there is no token, the identity is `"unknown"`.

It follows the same style, project layout, and conventions as
[`alpha-hack-program/elegibility-engine-mcp-rs`](https://github.com/alpha-hack-program/elegibility-engine-mcp-rs)
and this demo family's other Rust MCP servers (`mcp-portfolio`,
`mcp-market-news`, `mcp-crm-calendar`): `src/common/` + main-binary-with-
`rmcp` layout, JWT-decoded-without-signature-verification identity, a
`called_by`/`roles` response envelope, and matching Makefile/Containerfile
targets (UBI9 minimal, non-root).

## ⚠️ Disclaimer

This service's regulatory corpus (`data/corpus/*.md`) is a **simplified,
fictional** set of rules written for this demo. It does **not** reproduce
actual text from FATF, MiFID II, or any AML directive, and **must not be
used as a real regulatory reference**. `check_suitability`'s thresholds
(e.g. the 25% sector-concentration limit) are similarly fictional and
illustrative only — they are not a real compliance rule.

## Architecture

```
mcp-kyc-compliance/
├── src/
│   ├── common/
│   │   ├── auth.rs               # JWT payload decode (no signature check — Envoy does that)
│   │   ├── embedder.rs           # HTTP client for the shared vLLM/jina-embeddings-v3 service
│   │   ├── regulatory_corpus.rs  # loads data/corpus/*.md, builds/loads the TurboVec index, search()
│   │   ├── kyc_service.rs        # MCP service: the 3 tools, ToolResponse envelope, isolation rule
│   │   └── mod.rs
│   ├── bin/
│   │   └── corpus_indexer.rs     # standalone: manually (re)builds data/corpus.tv
│   ├── lib.rs                    # exposes `common` to the bins + tests
│   └── mcp_server.rs             # MCP server (rmcp, streamable-http)
└── data/
    └── corpus/                    # the 4 fictional regulatory markdown docs (tracked source)
```

At runtime, `data/corpus.tv` (the persisted TurboVec index) and
`data/corpus.tv.docs.json` (a small sidecar mapping index positions back to
`(source_file, text)`, since `TurboQuantIndex` only stores vectors) are
generated alongside `data/corpus/`. `data/` should be a PersistentVolumeClaim
in Kubernetes so this build isn't repeated on every pod restart.

## Identity extraction

Same pattern as every other MCP server in this demo family
(`src/common/auth.rs`, copied from `mcp-portfolio`): the gateway's Envoy
sidecar verifies the JWT signature *before* the request reaches this
process. This process only base64-decodes the JWT payload to read
`preferred_username` and `realm_access.roles` — it never verifies a
signature itself. Do not reuse this decoding logic anywhere the signature
hasn't already been validated upstream.

## Tools

| Tool | Parameters | Description |
|---|---|---|
| `get_risk_profile` | `client_id: String` | Declared risk profile, KYC status, and PEP flag for a client. Fails if the client doesn't belong to the caller. |
| `check_suitability` | `client_id: String`, `product_id: String` | Evaluates whether a product is potentially suitable for a client, per `data/corpus/03-suitability.md`. Ownership is validated only on `client_id` — `product_id` refers to a shared catalog, not client data. |
| `search_regulatory_guidance` | `query: String` | Semantic search over the fictional regulatory corpus. Returns the top matching text fragments plus their source document, so the caller can cite the clause. No ownership check applies. |

Every response is wrapped as:

```json
{ "output": { ... }, "called_by": "bob", "roles": ["banker"] }
```

`called_by`/`roles` are the audit trail — taken from the JWT, never from
the tool's own `output`.

### Isolation rule

Any tool that receives `client_id` first checks that the client belongs to
the authenticated banker (`assert_owns_client` in
`src/common/kyc_service.rs`, copied from `mcp-portfolio`'s implementation
of the same rule). If it doesn't belong to the caller, or doesn't exist at
all, the error is the same generic message either way — it never reveals
which case occurred. Every out-of-book access attempt is logged with
`tracing::warn!(target: "tenant_violation", ...)`.

`product_id` and `query` are **never** ownership-checked: `products` is a
shared catalog with no owner, and a free-text regulatory-guidance query has
no client to own.

### `check_suitability`'s concentration check is a proxy, not a simulation

The tool takes no investment-amount parameter (per spec), so it cannot
simulate the effect of a specific trade. Instead it computes the client's
**current** concentration in the product's own sector
(`sector market_value / total portfolio market_value`) and flags the
product as not-suitable-on-concentration-grounds if that current exposure
already exceeds 25% — i.e. "would this product further concentrate an
already-concentrated sector", not "would this specific trade push
concentration over the line". Risk-rating suitability
(`conservative ≤ moderate ≤ aggressive`, product must not exceed client) is
checked independently and is not a proxy.

## Embeddings: shared vLLM/KServe service

Same approach as the sibling `mcp-market-news` server — see that project's
README for the full rationale and migration history. In short:
`src/common/embedder.rs` is a thin `reqwest` client that calls a shared,
in-namespace vLLM/KServe `InferenceService` running `jinaai/jina-embeddings-v3`
on CPU (1024 dims) over its OpenAI-compatible `POST /v1/embeddings`
endpoint, normalized L2 client-side before use. This file is duplicated
(near-)verbatim from `mcp-market-news`'s own `src/common/embedder.rs` —
deliberately, not factored into a shared crate, same convention this demo
family already uses for `auth.rs` (the surviving code is thin enough that
duplication beats a shared crate's build/versioning overhead).

`turbovec::TurboQuantIndex::new(1024, 4)` (4-bit quantization — the corpus
here is tiny, 4 documents, so index size is a non-issue and 4 bits gives
the best recall of the supported widths).

### Self-healing index

Unlike `mcp-market-news`'s LLM-generated, periodically-reloaded corpus,
this corpus is static (four hand-authored markdown files baked into the
image). There is no periodic reload. Instead: `mcp_server` builds the index
itself on first startup if `data/corpus.tv` doesn't exist yet — no separate
initContainer needed, since indexing four short documents is fast once a
request to the embeddings service completes. The standalone `corpus_indexer`
binary (`make index-corpus`) exists only for manually rebuilding the index
after editing a corpus document, without starting the whole server.

## Environment variables

| Variable | Required | Description |
|---|---|---|
| `DATABASE_URL` | Yes | PostgreSQL connection string for the shared database, e.g. `postgres://user:pass@postgres.demo.svc.cluster.local:5432/demo` |
| `EMBEDDINGS_BASE_URL` | Yes | Base URL of the shared vLLM/KServe embeddings service, no trailing `/v1` (e.g. `http://jina-embeddings-v3-cpu-predictor.<namespace>.svc.cluster.local`) |
| `EMBEDDINGS_MODEL` | Yes | Served-model name to send in each `/v1/embeddings` request body (e.g. `jina-embeddings-v3-cpu`) |
| `CORPUS_DIR` | No | Directory of `*.md` corpus source files. Default `data/corpus` |
| `CORPUS_INDEX_PATH` | No | Path to the persisted TurboVec index. Default `data/corpus.tv` |
| `BIND_ADDRESS` | No | HTTP listen address. Default `127.0.0.1:8003` |
| `RUST_LOG` | No | `tracing` filter level (`debug`, `info`, `warn`, `error`). Default `info` |
| `MCP_DISABLE_HOST_CHECK` | No | `true` to disable streamable-http `Host` validation (useful for local curl testing) |
| `MCP_STATEFUL_MODE` | No | `true` to require session initialization before tool calls |
| `MCP_ALLOWED_HOSTS` | No | Comma-separated list of additional allowed `Host` headers |

## Database

The schema (shared with the other three MCP servers in this demo:
`mcp-portfolio`, `mcp-market-news`, `mcp-crm-calendar`) is **not** applied
by this binary. It lives in
`demos/keycloak-oidc/mcp-servers/templates/schema-init-configmap.yaml` and
is applied once per `helm install`/`upgrade` by that chart's
`schema-init-job.yaml` (a `post-install,post-upgrade` hook Job). On
startup, this service only checks that `clients`, `positions`, and
`products` exist — if not, it fails fast with a clear message instead of
failing confusingly on the first tool call.

`mcp-kyc-compliance` reads from `clients`, `positions`, and `products`; it
never writes to them.

## Build

```bash
make build   # cargo build --release --bins (mcp_server, corpus_indexer)
make test    # cargo test
make lint    # cargo clippy --all-targets -- -D warnings
make fmt     # cargo fmt --all
make audit   # cargo audit
```

## Run locally

```bash
export DATABASE_URL=postgres://demo:demo@localhost:5432/demo
export EMBEDDINGS_BASE_URL=http://localhost:8080
export EMBEDDINGS_MODEL=jina-embeddings-v3-cpu
export BIND_ADDRESS=0.0.0.0:8003
export MCP_DISABLE_HOST_CHECK=true  # local testing only, no Envoy in front
make run
```

## Testing with curl (unsigned fake JWT)

Same as the rest of this demo family: since this service only
base64-decodes the payload (Envoy validates the signature), an unsigned
JWT (`alg: none`) is enough for local testing.

```bash
PAYLOAD=$(echo -n '{"preferred_username":"bob","realm_access":{"roles":["banker"]}}' | base64 -w0 | tr '+/' '-_' | tr -d '=')
BOB_TOKEN="eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.${PAYLOAD}."

# Initialize an MCP session
curl -s http://127.0.0.1:8003/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{
    "jsonrpc": "2.0", "id": 1,
    "method": "initialize",
    "params": {
      "protocolVersion": "2025-03-26",
      "capabilities": {},
      "clientInfo": {"name": "curl-test", "version": "1.0"}
    }
  }'

# Should succeed: cli-001 is Bob's
curl -s http://127.0.0.1:8003/mcp \
  -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"get_risk_profile","arguments":{"client_id":"cli-001"}}}'

# Should fail: cli-005 belongs to Charlie, not Bob
curl -s http://127.0.0.1:8003/mcp \
  -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"get_risk_profile","arguments":{"client_id":"cli-005"}}}'

# Regulatory guidance search — no ownership check
curl -s http://127.0.0.1:8003/mcp \
  -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_regulatory_guidance","arguments":{"query":"when to escalate an unusual transaction"}}}'
```

## Container

```bash
podman build -t mcp-kyc-compliance:latest -f Containerfile .
podman run --rm -p 8003:8003 \
  -e DATABASE_URL=postgres://demo:demo@postgres:5432/demo \
  -e EMBEDDINGS_BASE_URL=http://jina-embeddings-v3-cpu-predictor.<namespace>.svc.cluster.local \
  -e EMBEDDINGS_MODEL=jina-embeddings-v3-cpu \
  mcp-kyc-compliance:latest
```

UBI9 minimal image, multi-stage build, non-root user `1001`. No C/C++
toolchain dependency at all (sqlx, reqwest, and turbovec are all pure
Rust, all configured for rustls) — a lighter build than the reference
repo's own requirements.

### Publication (GitHub Actions)

- **CI** (`.github/workflows/ci-mcp-kyc-compliance.yml`): on every push/PR
  touching `mcp/mcp-kyc-compliance/**`, runs `make check` (fmt + clippy)
  and `make test`.
- **Release** (`.github/workflows/release-mcp-kyc-compliance.yml`): on
  pushing a `mcp-kyc-compliance-v*` tag, repeats the check and publishes
  the image to `quay.io/atarazana/mcp-kyc-compliance` — the same registry
  already used by this demo's other MCP images — tagged both with the
  version (tag `mcp-kyc-compliance-v0.1.0` → image `0.1.0`) and `latest`.
  **Requires the repo secrets `REGISTRY_USER`/`REGISTRY_PASSWORD`**
  (`quay.io/atarazana` credentials), same as the sibling MCP servers.

```bash
git tag mcp-kyc-compliance-v0.1.0
git push origin mcp-kyc-compliance-v0.1.0
```

## What was verified

- `cargo build --release --bins`, `cargo test` (9 unit tests: JWT claim
  extraction, embedder normalization + fail-fast on missing env vars,
  corpus-directory loading/sorting, sidecar path handling), `cargo clippy
  --bins --tests -- -D warnings`, `cargo fmt --all -- --check`, and `cargo
  audit` (clean except the pre-existing, documented `paste` unmaintained
  warning — see `.cargo/audit.toml`) all pass in this sandbox.
- `podman build -t mcp-kyc-compliance:latest -f Containerfile .` succeeds
  end to end, including with only `gcc`/`make`/`pkg-config` in the builder
  (no C++ toolchain needed at all, unlike `mcp-market-news` before its own
  embeddings migration).
- The built image actually **runs** as non-root uid 1001 and reaches its
  first real I/O: pointed at an unreachable `DATABASE_URL`, it attempts the
  connection, respects `sqlx`'s pool-acquire timeout, and fails with a
  clear `pool timed out while waiting for an open connection` error rather
  than hanging silently or panicking uninformatively.
- **NOT verified**: a real request against a live Postgres database or a
  live `jinaai/jina-embeddings-v3` embeddings service. No such database or
  embeddings endpoint was reachable in the sandbox this server was built
  in — `assert_schema_ready`, the three tools' actual SQL, and
  `RegulatoryCorpus::load_or_build`'s real HTTP round-trip to the
  embeddings service are exercised only up to the point where they need
  one of those two live dependencies. Run the curl sequence above against
  a real deployment before considering this demo-ready.
- **NOT verified**: the exact shape/values of the 4 fictional corpus
  documents were not tuned against real embedding-model output (no
  reachable embeddings service) — `search_regulatory_guidance`'s actual
  retrieval quality (does querying "when to escalate" really surface
  `04-escalation.md` above the other three?) is unconfirmed. Worth
  checking once the shared embeddings service is reachable.

## Open risks

- **`check_suitability`'s 25%-concentration proxy has no companion "what if
  I invest $X" tool.** A banker using this tool to actually decide on a
  specific trade amount has no way to ask "would *this* trade push
  concentration over the line" — only "is the client already over the
  line in this sector". Acceptable for this demo's scope (matches the
  spec's tool signature exactly) but worth flagging if this ever needs to
  support a real advisory workflow.
- **Regulatory corpus retrieval quality is unvalidated** (see "What was
  verified" above) — `search_regulatory_guidance`'s usefulness as a
  citation tool depends on `jina-embeddings-v3` actually placing each
  corpus document's true topic close to the natural-language questions an
  agent would ask it; unconfirmed with 4 documents and no live embeddings
  service to test against.
- **Container not verified with a live embeddings service or Postgres in
  the loop** (see "What was verified") — only the fail-fast paths (schema
  check, embeddings HTTP client's own construction, unreachable-DB
  timeout) have been exercised inside the actual container.

## References

- Reference repo: https://github.com/alpha-hack-program/elegibility-engine-mcp-rs
- `turbovec`: https://docs.rs/turbovec
- OpenShell repo: https://github.com/NVIDIA/OpenShell
