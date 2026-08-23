IMAGE ?= mcp-crm-calendar:latest

.PHONY: build test lint fmt fmt-check check audit run image

build:
	cargo build --release

test:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

check: fmt-check lint test ## CI gate: formatting, clippy, tests

audit:
	cargo audit

run:
	cargo run --bin mcp_server

image:
	podman build -t $(IMAGE) -f Containerfile .
