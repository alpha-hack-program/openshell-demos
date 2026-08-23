IMAGE ?= mcp-crm-calendar:latest

.PHONY: build test lint fmt audit run image

build:
	cargo build --release

test:
	cargo test

lint:
	cargo clippy --all-targets --all-features -- -D warnings

fmt:
	cargo fmt --all

audit:
	cargo audit

run:
	cargo run --bin mcp_server

image:
	podman build -t $(IMAGE) -f Containerfile .
