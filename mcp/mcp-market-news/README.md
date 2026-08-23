# Market News MCP Server

> **Rust MCP server exposing public market-news retrieval, filtered by ticker/sector and (as a fallback) semantic similarity.**

This is one of four MCP servers in a banking demo family. Unlike the other
three (`mcp-portfolio`, `mcp-kyc-compliance`, `mcp-crm-calendar`), this
server does **NOT** apply per-client/per-caller data isolation: market news
is public data, not client data. Every tool response still carries
`called_by`/`roles`, for consistency with the rest of the family.

It follows the same style, project layout, and conventions as
[`alpha-hack-program/elegibility-engine-mcp-rs`](https://github.com/alpha-hack-program/elegibility-engine-mcp-rs)
— its `src/common/` + main-binary-with-`rmcp` layout, its
JWT-decoded-without-signature-verification identity pattern (a sidecar
Envoy verifies the signature upstream, so this process only reads claims),
its `called_by`/`roles` response convention, and its Makefile/Containerfile
targets (UBI9 minimal, non-root). That reference repo **was** reachable and
was cloned and inspected while building this server (see "What was
verified" below) — its `src/common/elegibility_engine.rs`, `Cargo.toml`,
`Makefile`, `Containerfile` and `README.md` were read directly.

## ⚠️ Disclaimer

Tickers, sectors, and news items in this demo are entirely fictional,
either hand-authored or LLM-generated for demonstration purposes. Nothing
here reflects real market events, real companies, or real financial advice.

## Architecture

```
mcp-market-news/
├── src/
│   ├── common/
│   │   ├── auth.rs           # JWT payload decode (no signature check — Envoy does that)
│   │   ├── embedder.rs       # candle-rs BERT (all-MiniLM-L6-v2), pure Rust, no ONNX
│   │   ├── news_service.rs   # get_relevant_news: two-stage filter logic
│   │   └── mod.rs
│   ├── bin/
│   │   └── news_generator.rs # batch job: Postgres + OpenAI-compatible LLM -> news.jsonl + news.tv
│   ├── lib.rs                 # exposes `common` to both bins + tests
│   └── mcp_server.rs          # MCP server (rmcp, streamable-http)
├── tests/
│   └── fixture.rs             # builds a local test corpus without Postgres/Anthropic (see below)
└── data/                       # PVC in Kubernetes; news.jsonl + news.tv live here at runtime
```

`mcp_server` loads `data/news.jsonl` fully into RAM and `data/news.tv`
(the persisted TurboVec index) once at startup, plus the embedder (needed
to embed stage-2 queries). The hot query path never touches disk again.

`news_generator` is a **separate batch binary** — run it before a demo
session, or schedule it as a Kubernetes `CronJob` against the same PVC.
It is not started by `mcp_server`.

## Identity extraction

Same pattern as the other three MCP servers in this demo family
(`src/common/auth.rs`): the gateway's Envoy sidecar verifies the JWT
signature *before* the request reaches this process. This process only
base64-decodes the JWT payload to read `preferred_username` and
`realm_access.roles` — it never verifies a signature itself. Do not reuse
this decoding logic anywhere the signature hasn't already been validated
upstream.

## The `get_relevant_news` tool

`get_relevant_news(tickers: Vec<String>, sectors: Vec<String>)` — called by
the agent after resolving a client's positions via `mcp-portfolio`, passing
that portfolio's tickers/sectors.

Two-stage logic, never returning the full feed:

1. **Stage 1 (cheap, always first):** exact-match filter over `data/news.jsonl`
   in RAM — ticker or sector, case-insensitive — within a 48h freshness
   window (a *relative* window, not an absolute date, so a corpus generated
   at any point still demos correctly as long as it's regenerated within
   that window).
2. **Stage 2 (only if stage 1 returns fewer than 2 results):** embeds a
   short natural-language query built from the given sectors
   (`"News affecting the {sectors} sector"` — see "Embedding query
   template" below for why it's phrased that way, not just the raw sector
   words) and searches the TurboVec index (`index.search(&query, 5)`),
   keeping only results with cosine similarity `> 0.6`.

Stage-1 and stage-2 hits are merged and de-duplicated by id.

Every response:

```rust
#[derive(Serialize)]
struct ToolResponse<T> { output: T, called_by: String, roles: Vec<String> }
```

## Embeddings: pure Rust, no ONNX, no external service

`src/common/embedder.rs` uses `candle-transformers`' native Rust BERT
implementation with `sentence-transformers/all-MiniLM-L6-v2` (384 dims).
Weights are fetched once via `hf-hub` (cached under `~/.cache/huggingface`,
or `$HF_HOME` — see the Containerfile, which points it at the same PVC as
the corpus) and mean-pooled + L2-normalized so a dot product between two
embeddings is a cosine similarity.

### Embedding query template

The spec this server was built from suggested embedding the raw sector
words directly (`sectors.join(" ")`). Empirically, on the tiny fixture
corpus used to test this server (see "What was verified" below), that
produced dangerously close scores between the true match and an unrelated
item: querying `"logistics"` scored the actual logistics item **lower**
(0.624) than an unrelated "utility company completes routine maintenance"
item (0.639) — a namespace collision in raw cosine, not a TurboVec
quantization artifact (confirmed identical on unquantized embeddings).
Wrapping the query in a short natural-language sentence —
`"News affecting the {sectors} sector"` — widened that gap to roughly 0.08
in the same test (0.634 vs. 0.549), which is what `news_service.rs`
actually does. See "Open risks" below — this was tuned against 6 items,
not the full ~40-item corpus the real generator would produce, and should
be re-validated once real data exists.

### TurboVec index

`turbovec::TurboQuantIndex::new(384, 4)` (4-bit quantization — the corpus
here is tiny, so index size is a non-issue and 4 bits gives the best
recall of the supported widths). Built by `news_generator`, loaded
read-only by `mcp_server` via `TurboQuantIndex::load("data/news.tv")`.

## `news_generator`: batch corpus generation

Run separately, before each demo session (or as a Kubernetes `CronJob`
against the same PVC `mcp_server` reads from) — **not** started by the
interactive service.

Pipeline (default `GENERATION_MODE=once`):

1. Reads distinct `(ticker, sector)` pairs from the shared portfolio
   Postgres database (`DATABASE_URL`), via `SELECT DISTINCT ticker, sector
   FROM positions`.
2. Asks an OpenAI-*compatible* `/v1/chat/completions` endpoint
   (`OPENAI_BASE_URL`, `OPENAI_API_KEY`, `OPENAI_MODEL`, via `reqwest` —
   plain HTTP, no SDK) for two batches:
   - **Batch 1 — background noise (35 items).**
   - **Batch 2 — two hand-guided seeded items**, so the demo always has
     something to find regardless of what's in the noise batch.
3. Embeds every item (`headline + body`) and adds the vectors to a
   `turbovec::TurboQuantIndex`.
4. Writes `data/news.jsonl` (source of truth, one JSON object per line)
   and `data/news.tv` (persisted index).

Chat Completions, not the Responses API `docs/inference-api-compatibility.md`
documents for Codex/namespace-tool compatibility — this call has no tool
use at all, and Chat Completions is the more broadly supported format
across self-hosted/third-party OpenAI-compatible endpoints (older vLLM
included). `OPENAI_BASE_URL` defaults to `https://api.openai.com` and does
**not** include a trailing `/v1` (same convention as that doc) — the code
appends `/v1/chat/completions` itself.

### Continuous mode (`GENERATION_MODE=loop`)

An alternative to running this binary as a one-shot job or Kubernetes
`CronJob`: set `GENERATION_MODE=loop` and it instead runs as a long-lived
process that generates a fresh drip-feed batch of `GENERATION_BATCH_SIZE`
items (default 5) immediately on startup, then again every
`GENERATION_INTERVAL_MINUTES` (default 5), forever — so a running demo
session sees "new" news arrive on its own without anyone re-running the
generator by hand. Each cycle:

- Uses the same `topup_prompt` shape as batch 1, just with a caller-supplied
  item count instead of the fixed 35, reusing `SELECT DISTINCT ticker,
  sector FROM positions` loaded once at startup.
- Appends the new items to the full in-memory corpus (seeded by reading
  back any existing `data/news.jsonl` on startup, so a restart resumes
  rather than starting the feed over) and rewrites `data/news.jsonl` +
  rebuilds `data/news.tv` in place via the exact same
  `write_jsonl`/`build_and_write_index` path the one-shot mode uses —
  `mcp_server` doesn't need to know which mode produced its corpus.
  Both writes are atomic (`{path}.tmp` + `rename()`), so a concurrent
  reader (see "Live reload in mcp_server" below) never observes a
  partially-written file.
- Logs and skips a failed cycle (LLM call, parsing, or write error) rather
  than crashing — a transient failure shouldn't take down a process meant
  to run unattended for an entire demo session.

**Live reload in `mcp_server`.** On its own, a `GENERATION_MODE=loop`
process only updates the files on disk — `mcp_server` needs to actually
notice. It does: a background task re-reads `news.jsonl`/`news.tv` every
`NEWS_RELOAD_INTERVAL_MINUTES` (default 5, matching the generator's own
default cadence; `0` disables it) and atomically swaps in the refreshed
corpus, so a `news_generator` sidecar's drip-feed reaches live traffic
without restarting `mcp_server`. The embedding model itself is loaded once
and reused across reloads (`NewsService::load_with_embedder`) — a reload
only re-reads the (tiny) corpus/index files, not the ~90MB model. See
"Environment variables" below for `NEWS_RELOAD_INTERVAL_MINUTES`.

Does **not** generate the two guaranteed seeded items (`NDFR` exact-ticker
hit, generic-logistics semantic hit) — those are a one-shot-mode-only
concern for bootstrapping a fresh corpus. Run `GENERATION_MODE=once` at
least once first if you need those guarantees present.

```bash
GENERATION_MODE=loop \
GENERATION_INTERVAL_MINUTES=5 \
GENERATION_BATCH_SIZE=5 \
  cargo run --release --bin news_generator
```

### Generation prompts (verbatim)

**Batch 1 — background noise (35 items):**

> Generate 35 short fictional financial news headlines for these
> tickers/sectors: {tickers_and_sectors}. Each item: `headline` (one
> sentence), `body` (2-3 sentences), `ticker` (can be null if
> sector-level), `sector`, `sentiment` (positive/negative/neutral). Most
> should be normal, low-impact market noise, not extraordinary events.
> Return only a JSON array, no extra text.

(`{tickers_and_sectors}` is rendered as a comma-separated list from the
`SELECT DISTINCT ticker, sector FROM positions` query.)

**Batch 2, seeded item 1 — guaranteed exact-ticker hit:**

> Generate 1 fictional financial news item explicitly mentioning ticker
> `NDFR` (logistics sector) describing a clear high-impact event (tariff
> change, operational incident, etc). Same format as above.

**Batch 2, seeded item 2 — guaranteed semantic-only hit:**

> Generate 1 fictional financial news item that does NOT mention any
> ticker by name, but describes an event generically affecting the
> "logistics" sector (e.g. a port regulatory change). This item tests
> semantic filtering, not exact ticker matching.

**Continuous mode (`GENERATION_MODE=loop`) drip-feed prompt** — same shape
as batch 1, `{count}` filled in from `GENERATION_BATCH_SIZE`:

> Generate {count} short fictional financial news headlines for these
> tickers/sectors: {tickers_and_sectors}. Each item: `headline` (one
> sentence), `body` (2-3 sentences), `ticker` (can be null if
> sector-level), `sector`, `sentiment` (positive/negative/neutral). Most
> should be normal, low-impact market noise, not extraordinary events.
> Return only a JSON array, no extra text.

Each generated item gets a real generation timestamp (`Utc::now()` at
generation time — not fabricated) and a fresh `uuid::Uuid::new_v4()` id.

## Environment variables

| Variable | Used by | Purpose |
|---|---|---|
| `DATABASE_URL` | `news_generator` | Postgres connection string for the shared `positions` table |
| `OPENAI_API_KEY` | `news_generator` | API key for the OpenAI-compatible chat-completions endpoint |
| `OPENAI_BASE_URL` | `news_generator` | Base URL, no trailing `/v1` (default `https://api.openai.com`); point at a self-hosted/vLLM endpoint to swap providers |
| `OPENAI_MODEL` | `news_generator` | Model name to request (required — no default, since it's provider-specific) |
| `GENERATION_MODE` | `news_generator` | `once` (default) for the one-shot batch, or `loop` for the continuous drip-feed service — see "Continuous mode" above |
| `GENERATION_INTERVAL_MINUTES` | `news_generator` | Loop mode only: minutes between drip-feed cycles (default `5`) |
| `GENERATION_BATCH_SIZE` | `news_generator` | Loop mode only: items generated per cycle (default `5`) |
| `NEWS_JSONL_PATH` | both | Path to the corpus (default `data/news.jsonl`) |
| `NEWS_TV_PATH` | both | Path to the TurboVec index (default `data/news.tv`) |
| `NEWS_RELOAD_INTERVAL_MINUTES` | `mcp_server` | Minutes between live corpus reloads from disk (default `5`); `0` disables reload entirely — see "Live reload in mcp_server" above |
| `BIND_ADDRESS` | `mcp_server` | Listen address (default `127.0.0.1:8002`) |
| `MCP_DISABLE_HOST_CHECK` | `mcp_server` | Set `true`/`1` for local/curl testing (disables the streamable-http allowed-hosts check) |
| `MCP_STATEFUL_MODE` | `mcp_server` | Set `true`/`1` to require session initialization before tool calls |
| `MCP_ALLOWED_HOSTS` | `mcp_server` | Comma-separated extra allowed `Host` headers |
| `HF_HOME` | both (via `hf-hub`) | Hugging Face cache root; point at the corpus PVC in Kubernetes so the model download survives restarts |
| `RUST_LOG` | both | `tracing` filter, e.g. `info`, `debug` |

## Testing

### Unit tests (no network, no external services)

```bash
cargo test
```

12 unit tests cover: JWT claim extraction (`auth.rs`), stage-1 exact-match
filtering including case-insensitivity and freshness-window rejection,
JSONL parsing, and the LLM-response JSON extraction logic in
`news_generator` (clean array, array wrapped in prose/code fences, single
object for the seed prompts).

### Building a local test corpus (no Postgres, no LLM API key needed)

`news_generator`'s own pipeline needs a live Postgres `positions` table
and `OPENAI_API_KEY` — neither was available in the environment this
server was built in (see "What was verified" below). `tests/fixture.rs`
exercises the *same* embedding + indexing code (`Embedder`,
`TurboQuantIndex`) against a small hand-authored corpus instead, so the
server can actually be started and queried end-to-end:

```bash
cargo test --release --test fixture -- --ignored --nocapture
```

This downloads `sentence-transformers/all-MiniLM-L6-v2` from the Hugging
Face Hub on first run (~90MB, cached afterwards) and writes
`data/news.jsonl` + `data/news.tv`.

### Running the server and calling the tool

```bash
# 1. Produce data/news.jsonl + data/news.tv (real generator or test fixture, above)
# 2. Start the service
MCP_DISABLE_HOST_CHECK=true RUST_LOG=info cargo run --release --bin mcp_server
```

```bash
PAYLOAD=$(echo -n '{"preferred_username":"bob","realm_access":{"roles":["banker"]}}' | base64 -w0 | tr '+/' '-_' | tr -d '=')
BOB_TOKEN="eyJhbGciOiJub25lIiwidHlwIjoiSldUIn0.${PAYLOAD}."

curl -s http://127.0.0.1:8002/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  -H "Authorization: Bearer $BOB_TOKEN" \
  --data '{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"get_relevant_news","arguments":{"tickers":["NDFR"],"sectors":["logistics"]}}}'
```

Expected shape:

```json
{
  "output": [ { "id": "...", "headline": "...", "...": "..." } ],
  "called_by": "bob",
  "roles": ["banker"]
}
```

With no `Authorization` header, `called_by` is `"unknown"` and `roles` is
`[]`.

## Configuration reference

See `Cargo.toml` for pinned dependency versions. Notably:

- `rmcp = "1.7"` — same major version as the reference repo.
- `hf-hub = "1.0"` — migrated off the `0.4.3` stopgap pin; see "What was
  verified" below for what the migration actually involved.
- `reqwest`/`hf-hub` are configured for **rustls**, not the default
  native-tls/openssl backend, so building doesn't require a system OpenSSL
  install (only a C++ compiler, for `tokenizers`' `esaxx-rs`).

## Makefile targets

`build`, `test`, `test-network` (the `#[ignore]`d, network-gated tests),
`lint` (`clippy -D warnings`), `fmt`/`fmt-check`, `audit` (`cargo-audit`,
requires `cargo install cargo-audit`), `generate-news` (runs
`news_generator`), `run` (runs `mcp_server`), `image-build`/`image-run`
(podman), `clean`.

## Security

- Non-root container user `1001` (same convention as the rest of this demo
  family).
- No JWT signature verification in-process — by design; the Envoy sidecar
  in front of this service does that. This process must never be exposed
  directly without that sidecar.
- `cargo audit` clean except two documented-ignore entries — see
  `.cargo/audit.toml`.

## What was verified

This project's toolchain and dependency versions were checked against
live sources, not assumed from training-data priors, because several of
them (candle-transformers, hf-hub, turbovec, rmcp) have had breaking API
changes across versions:

- **Reference repo**: cloned via `git clone` (network was reachable) and
  read directly — `Cargo.toml`, `src/mcp_server.rs`, `src/common/*.rs`,
  `Makefile`, `Containerfile`, `README.md`.
- **`turbovec` 1.0.0**: API (`TurboQuantIndex::new/add/search/write/load`,
  `SearchResults::scores_for_query/indices_for_query`) read directly from
  the published source on docs.rs — no `ef` search parameter exists in
  the real crate (the spec's illustrative `search(&query, top_k, ef=50)`
  doesn't match; the real signature is `search(&self, queries: &[f32], k:
  usize)`).
- **`candle-transformers`/`candle-nn` 0.11.0**: `BertModel::load`,
  `Config`, `DTYPE`, and `VarBuilder::from_mmaped_safetensors` signatures
  read directly from source — matched the spec's illustrative snippet.
- **`hf-hub` 1.0.0 migration (2026-08-23)**: this project previously pinned
  `0.4.3` as a stopgap because `1.0.0` is a *complete rewrite* (`HFClient`-
  based, async-first) with no `api::sync::Api` module at all. It has since
  been migrated to `1.0` proper, confirmed by actually building and
  running against it, not just reading docs.rs:
  - Cargo.toml: `default-features = false, features = ["ureq"]` (the
    `ureq` feature doesn't exist in 1.0 — it hard-depends on `reqwest`
    now) became `features = ["blocking", "rustls-tls"]`. `blocking` is
    required to get any synchronous API at all; `rustls-tls` maps to
    `reqwest/rustls`, same rationale as this project's own `reqwest`
    dependency above.
  - `src/common/embedder.rs`: `hf_hub::api::sync::Api::new()` /
    `api.model("org/name")` / `repo.get("file")` became
    `hf_hub::HFClientSync::new()` / `client.model("org", "name")` (owner
    and name are now two separate arguments, not one slash-joined string)
    / `repo.download_file().filename("file").send()` (builder pattern).
    `.send()`'s `HFResult<PathBuf>` doesn't implement `std::error::Error`
    in a way `?` composes with directly against `anyhow::Result`, so each
    call is `.map_err(|e| anyhow::anyhow!(e.to_string()))?`'d, same
    pattern already used for `tokenizers`' error type in this file.
  - The planned diff needed **no changes** beyond that — no additional
    compiler errors turned up once the above was in place.
    `cargo build --release --bins`, `cargo check`, `cargo clippy --bins
    --tests -- -D warnings`, and `cargo fmt --check` all pass clean.
  - **`hf-xet` needed nothing extra.** It, `bon`, `globset`,
    `tokio-retry`, `sha2`, and `hyper` all pulled in and built fine with
    only the `gcc-c++` this project already required for `tokenizers`'
    `esaxx-rs` — no new system packages, on the host. The container image
    has **not** been rebuilt against this heavier dependency tree yet —
    see "Open risks".
  - **The nested-tokio-runtime claim holds.** `Embedder::load()` is called
    synchronously from inside `#[tokio::main] async fn main()` in
    `mcp_server.rs`. `HFClientSync::download_file().send()` — which
    internally runs `hf-xet`'s transfer machinery on its own background
    thread/runtime — was called from exactly that context (via
    `tests/fixture.rs` and via `mcp_server` actually starting and serving
    requests) without a "cannot start a runtime from within a runtime"
    panic or any restructuring of `main()`.
  - **Verified against a real Hugging Face download, twice, on the host**:
    once against a pre-existing local HF cache (fast, cache-hit path), and
    once with `HF_HOME` pointed at an empty temp directory to force a
    genuine fresh download (~88MB landed in `<HF_HOME>/hub/...` plus an
    `<HF_HOME>/xet/` staging directory used by `hf-xet`'s chunked
    transfer) — both the `#[ignore]`d unit test in `embedder.rs` and
    `tests/fixture.rs` passed in both cases. This also confirms
    `HFClientSync::new()` honors `HF_HOME` correctly out of the box —
    superseding the `ApiBuilder::from_env()` workaround the 0.4.3-era
    code needed (see the retired bug note below).
  - `cargo test` (12 unit tests, none touching `hf-hub`) still passes
    unchanged.
  - `mcp_server` was started on the host (`cargo run --release --bin
    mcp_server`) against the corpus `tests/fixture.rs` built, and
    re-queried live with the three `curl` cases from "Testing"/"What was
    verified" above (exact ticker match, semantic-only match, no-match) —
    all three returned the same expected shapes as before the migration.
  - `cargo audit` is unaffected by this change (one pre-existing,
    unrelated `paste` "unmaintained" advisory warning, plus the
    already-documented `RUSTSEC-2023-0071` ignore in
    `.cargo/audit.toml` — neither is new from this migration).
- **LLM provider swapped from Anthropic to OpenAI-compatible (2026-08-23)**:
  `news_generator` originally called the Anthropic Messages API directly
  (model `claude-sonnet-4-6`). It now calls a generic OpenAI-compatible
  `/v1/chat/completions` endpoint instead (`OPENAI_BASE_URL`/
  `OPENAI_API_KEY`/`OPENAI_MODEL`), matching the env var convention already
  used elsewhere in this repo for BYO-LLM setups (see
  `docs/inference-api-compatibility.md`) — so any self-hosted vLLM route or
  third-party OpenAI-compatible provider works, not just a paid Anthropic
  key. Chat Completions specifically, not the Responses API that same doc
  documents for Codex/namespace-tool compatibility, since this call does no
  tool use at all and Chat Completions is the more broadly supported format
  (older vLLM included). Compiles and passes `cargo check`/`clippy`/`fmt`
  clean; the live call itself is unverified (see "NOT verified" below —
  same constraint as before the swap, just a different missing credential).
- **Both binaries actually compile and link**: `cargo check --bins`,
  `cargo build --release --bins`, `cargo clippy --bins --tests -D
  warnings`, and `cargo fmt --check` all pass clean in this environment
  (two system packages had to be installed first: `gcc-c++`, for
  `tokenizers`' `esaxx-rs`; and `cargo-audit`, which isn't preinstalled).
- **`mcp_server` actually ran and was queried live**: `tests/fixture.rs`
  built a real 6-item corpus (real HF model download, real embeddings,
  real TurboVec index) and the running server was hit with real `curl`
  requests over `/mcp` — see the three cases in "Testing" and in "Open
  risks" below. All three (exact ticker match, semantic-only match with
  no ticker, and a no-match case) returned the expected narrowed results
  with `called_by`/`roles` attached.
- **NOT verified**: `news_generator`'s actual Postgres query and LLM API
  call (both `once` and `loop` mode) were never executed — this sandbox had
  no `DATABASE_URL`, no live Postgres `positions` table, and no
  `OPENAI_API_KEY`. The code compiles and its JSON-parsing logic is
  unit-tested against representative LLM output shapes (clean array, array
  wrapped in prose, single object), but the live LLM/Postgres calls
  themselves and the loop's sleep/retry cycle are unverified beyond
  compiling and passing `clippy`/`fmt`/`cargo check`.
- **`mcp_server`'s live reload actually verified end to end** (2026-08-23):
  started `mcp_server` against the `tests/fixture.rs` corpus with
  `NEWS_RELOAD_INTERVAL_MINUTES=1`, confirmed a query for a ticker not yet
  in the corpus returned empty, appended a new item with that ticker
  directly to `data/news.jsonl`/`data/news.tv` (atomically, same
  write-then-rename approach `news_generator` uses) while the server kept
  running, waited past the 1-minute mark, and confirmed both the
  `"Reloaded market news corpus"` log line and a re-query now returning
  the new item — all without restarting the process. This is real
  verification of the reload mechanism itself (the `RwLock<Arc<NewsService>>`
  swap, the atomic-write race-avoidance, `load_with_embedder` reusing the
  already-loaded model); it does not substitute for verifying the
  `news_generator` sidecar's own live LLM call, which remains unverified
  per the bullet above.
- **Containerfile actually built and run against the `hf-hub 0.4.3`-era
  code** (`podman`, invoked via `flatpak-spawn --host podman` — this
  sandbox is a toolbox container where the in-container `podman` is
  non-functional, but the *host* podman works fine through
  `flatpak-spawn`): `podman build` succeeded end to end (245MB final
  image), and `podman run` with `data/` bind-mounted (`:Z`) actually
  started the server as non-root uid 1001 (confirmed via `podman exec
  ... id`) and served real `curl` requests over `/mcp` — same three
  cases as the host-run test above, all correct. Two real,
  container-specific bugs were caught this way and fixed before
  considering it done (see below) — this is exactly why "the code
  compiles" and "it actually runs as intended" are different claims.
  **This container verification predates the `hf-hub` 1.0 migration and
  has not been re-run since** — the migration was only verified on the
  host (see above). Given `hf-hub` 1.0 pulls in a noticeably heavier
  dependency tree (`hf-xet`, `bon`, `globset`, `tokio-retry`, `sha2`,
  `hyper`), re-running the full `podman build` + `run` + `curl` cycle
  before shipping this is the single most important follow-up — see
  "Open risks".
- **Retired bug (pre-migration, `hf-hub 0.4.3`): `Embedder::load()`
  ignored `HF_HOME` entirely.** `hf_hub::api::sync::Api::new()` called
  `ApiBuilder::new()`, which built its cache from `Cache::default()`
  (`dirs::home_dir()`), **not** `Cache::from_env()` — `HF_HOME` was only
  honored by `ApiBuilder::from_env()`, a different constructor. This
  "worked" under plain `cargo run`/`cargo test` on the host purely by
  accident, because `$HOME` already had the model cached there from a
  previous run. It failed loudly the moment this ran as the container's
  non-root user, whose `$HOME` (`/home/mcpserver`) doesn't exist and
  isn't writable: `Permission denied (os error 13)` out of
  `Embedder::load`, with the `HF_HOME` env var pointing at the mounted
  PVC being silently ignored the whole time. Root-caused with a minimal
  reproduction binary built into the same UBI9 runtime image, fixed at
  the time by switching to `ApiBuilder::from_env()`, then re-verified
  with a full rebuild + container run + curl round-trip. **This code
  path no longer exists** — the `hf-hub` 1.0 migration replaced it with
  `HFClientSync::new()`, independently confirmed above to honor
  `HF_HOME` correctly via a real cold-cache download test. Kept here as
  a record of why the container test mattered: without it, this class of
  bug would have shipped invisibly, since `cargo test`/`cargo run` on a
  normal dev machine can't surface a `$HOME`-doesn't-exist condition.
- **Real bug found and fixed: the `HEALTHCHECK` would have falsely
  failed.** Discovered by actually curling the running server: `GET
  /mcp` returns `405` (it's a POST-only JSON-RPC endpoint), which `curl
  -f` treats as failure. Fixed by dropping `-f` so the healthcheck only
  checks TCP-level reachability. Still current — the migration didn't
  touch the Containerfile. (Also confirmed, by inspecting the build log,
  that `ubi9-minimal`'s default `ca-certificates` install does ship a
  working `curl` — `curl-minimal` — so no extra package was needed for
  this to work; `podman` itself separately warned that `HEALTHCHECK` is
  ignored for OCI-format images and needs `--format docker` to actually
  take effect, which is a `podman build`-time concern, not a
  Containerfile bug.)

## Open risks

- **Similarity threshold (`0.6`) is unvalidated against the real ~40-item
  corpus.** It was tuned against a 6-item hand-authored fixture (see
  "Embedding query template" above), where an unrelated "utility
  maintenance" item scored uncomfortably close to (and, with the naive
  raw-sector-words query, above) the true logistics match. The natural-
  language query template (`"News affecting the {sectors} sector"`)
  fixed the observed case, but this is a small sample — re-run
  `tests/fixture.rs`-style scoring diagnostics against the real generated
  corpus once `news_generator` has actually been run against live data,
  and tune `SIMILARITY_THRESHOLD` / `MIN_STAGE1_HITS` in
  `src/common/news_service.rs` if false positives/negatives show up at
  that scale.
- **`news_generator`'s Postgres query and Anthropic call are untested
  end-to-end.** The `positions` table schema is assumed from the spec
  (`ticker`, `sector` columns) — it wasn't cross-checked against
  `mcp-portfolio`'s actual schema (a sibling MCP server built in a
  separate, concurrent session this project intentionally did not touch).
  If that schema differs, `load_tickers_and_sectors` in
  `src/bin/news_generator.rs` will need adjusting.
- **Container not re-verified against `hf-hub 1.0`.** The `podman build`
  + `run` + `curl` cycle documented above was run against the `0.4.3`-era
  code; the `1.0` migration was only verified on the host. `hf-hub 1.0`
  pulls in a noticeably heavier dependency tree (`hf-xet`, `bon`,
  `globset`, `tokio-retry`, `sha2`, `hyper`) — worth specifically
  checking that `hf-xet`'s transfer machinery (which spins up its own
  multi-threaded runtime for Xet uploads/downloads) behaves the same
  under the container's non-root user and restricted `$HOME` as it did
  on the host, and that the final image size hasn't grown enough to
  matter. Re-run the full container cycle before considering this demo
  ready.
- **`podman build --format docker` not yet tried.** The image builds and
  runs correctly, but `podman` warned that `HEALTHCHECK` is ignored for
  the default OCI image format. If the `HEALTHCHECK` needs to actually
  take effect (vs. just being present for humans/tooling reading the
  Containerfile), build with `--format docker` or rely on a Kubernetes
  liveness/readiness probe instead, which is the more idiomatic choice
  for this demo's actual OpenShift target anyway.

## References

- Reference repo: https://github.com/alpha-hack-program/elegibility-engine-mcp-rs
- `turbovec`: https://docs.rs/turbovec
- OpenShell repo: https://github.com/NVIDIA/OpenShell
