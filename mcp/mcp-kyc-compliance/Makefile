.PHONY: build test lint fmt fmt-check audit check ci run index-corpus image-build clean help \
        release-patch release-minor release-major release-dry-run

build:
	cargo build --release --bins

test:
	cargo test

lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all -- --check

audit:
	cargo audit

check: fmt-check lint

ci: check test

run: build
	RUST_LOG=info ./target/release/mcp_server

# Rebuilds data/corpus.tv (+ its doc sidecar) from data/corpus/*.md.
# mcp_server does this itself on first startup if the index is missing, so
# running this manually is optional — useful after editing a corpus
# document, or to warm the index ahead of time in CI/local dev.
index-corpus: build
	RUST_LOG=info ./target/release/corpus_indexer

image-build:
	podman build -t mcp-kyc-compliance:latest -f Containerfile .

clean:
	cargo clean

# Release management with cargo-release. Tags as mcp-kyc-compliance-vX.Y.Z
# and pushes, which triggers .github/workflows/release-mcp-kyc-compliance.yml.
release-patch:
	@echo "Creating patch release (x.y.Z+1)..."
	cargo release patch --execute

release-minor:
	@echo "Creating minor release (x.Y+1.0)..."
	cargo release minor --execute

release-major:
	@echo "Creating major release (X+1.0.0)..."
	cargo release major --execute

release-dry-run:
	@echo "Dry run - showing what would happen..."
	cargo release patch --dry-run

help:
	@echo "Usage:"
	@echo "  make build         - Build both binaries (mcp_server, corpus_indexer)"
	@echo "  make test          - Run cargo test"
	@echo "  make lint          - Run cargo clippy (warnings as errors)"
	@echo "  make fmt           - Format code with cargo fmt"
	@echo "  make fmt-check     - Check formatting without modifying files"
	@echo "  make audit         - Run cargo audit"
	@echo "  make check         - Run fmt-check and lint (CI gate)"
	@echo "  make ci            - Alias for check + test"
	@echo "  make run           - Build and run mcp_server locally"
	@echo "  make index-corpus  - Rebuild data/corpus.tv from data/corpus/*.md"
	@echo "  make image-build   - Build the container image"
	@echo "  make clean         - Remove build artifacts"
	@echo ""
	@echo "Release Commands (uses cargo-release):"
	@echo "  make release-patch   - Create patch release (0.1.0 -> 0.1.1)"
	@echo "  make release-minor   - Create minor release (0.1.0 -> 0.2.0)"
	@echo "  make release-major   - Create major release (0.1.0 -> 1.0.0)"
	@echo "  make release-dry-run - Show what release-patch would do"
