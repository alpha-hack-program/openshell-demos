.PHONY: build build-server build-generator test lint fmt fmt-check check audit generate-news run clean image-build image-run help \
        release-patch release-minor release-major release-dry-run

all: build

# Build both binaries in release mode
build:
	cargo build --release --bins

build-server:
	cargo build --release --bin mcp_server

build-generator:
	cargo build --release --bin news_generator

# Run all tests. The embedder/fixture tests that download the HF model are
# #[ignore]d by default — see `test-network` to run them explicitly.
test:
	cargo test

# Network-gated tests: downloads sentence-transformers/all-MiniLM-L6-v2 from
# Hugging Face Hub on first run, and writes a local data/news.jsonl +
# data/news.tv fixture (see tests/fixture.rs for what it seeds).
test-network:
	cargo test --release -- --ignored --nocapture

lint:
	cargo clippy --bins --tests -- -D warnings

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

# Requires cargo-audit: `cargo install cargo-audit`. See .cargo/audit.toml
# for the one documented, justified ignore (RUSTSEC-2023-0071, a
# not-actually-linked transitive dep — see that file for the reasoning).
audit:
	cargo audit

# Batch corpus generation. Requires DATABASE_URL (shared portfolio Postgres)
# and ANTHROPIC_API_KEY. Run this before starting mcp_server — it is NOT
# started automatically by the service. See README.md for the exact prompts
# used and for how to build a local test fixture instead when those two
# env vars aren't available.
generate-news: build-generator
	./target/release/news_generator

# Run the MCP server locally (expects data/news.jsonl + data/news.tv to
# already exist — run `make generate-news` first).
run: build-server
	MCP_DISABLE_HOST_CHECK=true RUST_LOG=info ./target/release/mcp_server

clean:
	cargo clean

image-build:
	podman build -t mcp-market-news:latest -f Containerfile .

image-run:
	podman run --rm -p 8002:8002 \
		-v "$$(pwd)/data:/app/data:Z" \
		-e RUST_LOG=info \
		mcp-market-news:latest

help:
	@echo "Usage:"
	@echo ""
	@echo "  make build            - Build both binaries (release)"
	@echo "  make build-server     - Build mcp_server only"
	@echo "  make build-generator  - Build news_generator only"
	@echo "  make test             - Run unit tests (network-gated tests skipped)"
	@echo "  make test-network     - Run the ignored tests (downloads HF model)"
	@echo "  make lint             - cargo clippy -D warnings"
	@echo "  make fmt              - cargo fmt"
	@echo "  make fmt-check        - cargo fmt --check"
	@echo "  make audit            - cargo audit (requires cargo-audit)"
	@echo "  make generate-news    - Run the news_generator batch job"
	@echo "  make run              - Run mcp_server locally"
	@echo "  make image-build      - Build the mcp_server container image"
	@echo "  make image-run        - Run the container image locally"
	@echo "  make clean            - cargo clean"
