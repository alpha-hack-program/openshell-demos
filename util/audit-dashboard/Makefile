.DEFAULT_GOAL := help

BIN          := audit-dashboard

IMAGE_REGISTRY ?= quay.io/atarazana
# Same version cargo-release would bump to and tag as audit-dashboard-v<version>
# (see release.toml/release-patch below) — keeps a locally built/pushed image
# tag matching what's actually in Cargo.toml, not a moving "latest".
CARGO_VERSION  := $(shell awk -F'"' '/^version/ {print $$2; exit}' Cargo.toml)
IMAGE_TAG      ?= $(CARGO_VERSION)
IMAGE          := $(IMAGE_REGISTRY)/$(BIN):$(IMAGE_TAG)

# Bare `podman` fails inside this project's toolbox (nested user-namespace/
# pause-process error) — re-exec against the host's podman via flatpak-spawn
# when running from inside one.
ifneq ($(wildcard /run/.toolboxenv),)
PODMAN := flatpak-spawn --host podman
else
PODMAN := podman
endif

.PHONY: help build release install run watch \
        fmt fmt-check clippy check ci \
        frontend clean doc \
        image-build image-push \
        release-patch release-minor release-major

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

build: ## Debug build (backend only — frontend/dist/ is only needed to actually serve requests)
	cargo build

release: ## Release build (native target)
	cargo build --release

install: release ## Build release and install to ~/bin
	install -d $(HOME)/bin
	install -m 755 target/release/$(BIN) $(HOME)/bin/$(BIN)

frontend: ## Build the Preact frontend into frontend/dist (npm ci + npm run build)
	cd frontend && npm ci && npm run build

run: frontend ## Build the frontend, then run the backend in debug mode against ./frontend/dist
	cd frontend && ln -sfn dist ../dist 2>/dev/null || true
	cargo run -- $(ARGS)

fmt: ## Format the code
	cargo fmt

fmt-check: ## Check formatting without modifying files
	cargo fmt --check

clippy: ## Lint with clippy, warnings as errors
	cargo clippy --all-targets -- -D warnings

check: fmt-check clippy ## Run fmt-check and clippy (no test suite yet)

ci: check ## Alias for check, matches CI gate

clean: ## Remove build artifacts
	cargo clean
	rm -rf frontend/node_modules frontend/dist dist

doc: ## Build and open docs
	cargo doc --open --no-deps

image-build: ## Build the container image, tagged with Cargo.toml's version (override with IMAGE_TAG=...)
	# No local `frontend` prerequisite here on purpose — the Containerfile
	# builds the Preact bundle itself in its own frontend-builder stage, so
	# `podman build` alone is self-sufficient even on a machine with no
	# Node installed at all (confirmed live: this repo's own build
	# environment has cargo/podman but no node in the same shell).
	$(PODMAN) build -t $(IMAGE) -f Containerfile .

image-push: image-build ## Build and push the container image (override registry with IMAGE_REGISTRY=...)
	$(PODMAN) push $(IMAGE)

## --- Release management with cargo-release ---
## Bumps Cargo.toml version, runs `make ci` as a gate (see release.toml),
## commits, tags audit-dashboard-v<version>, and pushes. Never publishes to
## crates.io.

release-patch: ## Bump patch version (x.y.Z+1), tag, and push
	@echo "Creating patch release (x.y.Z+1)..."
	cargo release patch --execute

release-minor: ## Bump minor version (x.Y+1.0), tag, and push
	@echo "Creating minor release (x.Y+1.0)..."
	cargo release minor --execute

release-major: ## Bump major version (X+1.0.0), tag, and push
	@echo "Creating major release (X+1.0.0)..."
	cargo release major --execute
