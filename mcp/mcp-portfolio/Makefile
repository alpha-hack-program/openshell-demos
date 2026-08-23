.PHONY: build test lint fmt audit run image-build clean help

build:
	cargo build --release --bin mcp_server

test:
	cargo test

lint:
	cargo clippy --all-targets -- -D warnings

fmt:
	cargo fmt --all

audit:
	cargo audit

run: build
	RUST_LOG=info ./target/release/mcp_server

image-build:
	podman build -t mcp-portfolio:latest -f Containerfile .

clean:
	cargo clean

help:
	@echo "Usage:"
	@echo "  make build        - Build the mcp_server release binary"
	@echo "  make test         - Run cargo test"
	@echo "  make lint         - Run cargo clippy (warnings as errors)"
	@echo "  make fmt          - Format code with cargo fmt"
	@echo "  make audit        - Run cargo audit"
	@echo "  make run          - Build and run mcp_server locally"
	@echo "  make image-build  - Build the container image"
	@echo "  make clean        - Remove build artifacts"
