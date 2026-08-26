IMAGE ?= mcp-crm-calendar:latest

.PHONY: build test lint fmt fmt-check check audit run image \
        release-patch release-minor release-major release-dry-run

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

## --- Release management with cargo-release ---
## Bumps Cargo.toml version, runs `make check` as a gate (see release.toml),
## commits, tags mcp-crm-calendar-v<version>, and pushes. Never publishes to
## crates.io. GitHub Actions then builds and pushes the image (see
## .github/workflows/release-mcp-crm-calendar.yml).

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
