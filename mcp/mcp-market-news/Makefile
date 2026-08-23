.PHONY: build build-server build-generator test lint fmt fmt-check check audit generate-news generate-news-loop run clean image-build image-run help \
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

check: fmt-check lint test ## CI gate: formatting, clippy, tests

# Requires cargo-audit: `cargo install cargo-audit`. See .cargo/audit.toml
# for the one documented, justified ignore (RUSTSEC-2023-0071, a
# not-actually-linked transitive dep — see that file for the reasoning).
audit:
	cargo audit

# One-shot batch corpus generation. Requires DATABASE_URL (shared portfolio
# Postgres), OPENAI_API_KEY, and OPENAI_MODEL (OPENAI_BASE_URL defaults to
# api.openai.com). Run this before starting mcp_server — it is NOT started
# automatically by the service. See README.md for the exact prompts used
# and for how to build a local test fixture instead when those env vars
# aren't available.
generate-news: build-generator
	./target/release/news_generator

# Continuous drip-feed mode: generates GENERATION_BATCH_SIZE items every
# GENERATION_INTERVAL_MINUTES, forever, starting immediately. Same env vars
# as generate-news. See README.md ("Continuous mode") for details.
generate-news-loop: build-generator
	GENERATION_MODE=loop ./target/release/news_generator

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

## --- Release management with cargo-release ---
## Bumps Cargo.toml version, runs `make check` as a gate (see release.toml),
## commits, tags mcp-market-news-v<version>, and pushes. Never publishes to
## crates.io. GitHub Actions then builds and pushes the image (see
## .github/workflows/release-mcp-market-news.yml).

release-patch: ## Bump patch version (x.y.Z+1), tag, and push
	@echo "Creating patch release (x.y.Z+1)..."
	cargo release patch --execute

release-minor: ## Bump minor version (x.Y+1.0), tag, and push
	@echo "Creating minor release (x.Y+1.0)..."
	cargo release minor --execute

release-major: ## Bump major version (X+1.0.0), tag, and push
	@echo "Creating major release (X+1.0.0)..."
	cargo release major --execute

release-dry-run: ## Show what release-patch would do, without executing it
	@echo "Dry run - showing what would happen..."
	cargo release patch --dry-run

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
	@echo "  make check            - CI gate: fmt-check + lint + test"
	@echo "  make audit            - cargo audit (requires cargo-audit)"
	@echo "  make generate-news    - Run the news_generator batch job (one-shot)"
	@echo "  make generate-news-loop - Run news_generator as a continuous drip-feed service"
	@echo "  make run              - Run mcp_server locally"
	@echo "  make image-build      - Build the mcp_server container image"
	@echo "  make image-run        - Run the container image locally"
	@echo "  make clean            - cargo clean"
	@echo "  make release-patch    - Bump patch version, tag, and push"
	@echo "  make release-minor    - Bump minor version, tag, and push"
	@echo "  make release-major    - Bump major version, tag, and push"
	@echo "  make release-dry-run  - Show what release-patch would do"
