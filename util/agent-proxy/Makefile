.DEFAULT_GOAL := help

BIN            := agent-proxy
MUSL_TARGET    := x86_64-unknown-linux-musl
MUSL_BIN       := target/$(MUSL_TARGET)/release/$(BIN)
GARAK_IMAGE_REGISTRY ?= quay.io/atarazana
CODEX_IMAGE_DIR  := ../../demos/keycloak-oidc/images/codex-garak
CLAUDE_IMAGE_DIR := ../../demos/keycloak-oidc/images/claude-garak

.PHONY: help build release musl \
        fmt fmt-check clippy check ci \
        clean doc \
        image-codex image-claude push-codex push-claude push \
        release-patch release-minor release-major

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

build: ## Debug build (native target)
	cargo build

release: ## Release build (native target — NOT the deployable artifact, see musl)
	cargo build --release

# Sandbox base images ship an older glibc than most dev machines (confirmed:
# Fedora 44's glibc 2.41 is too new); musl static-links around that entirely.
musl: ## Build a static musl binary — the artifact actually deployed into sandboxes
	cargo build --release --target $(MUSL_TARGET)

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

doc: ## Build and open docs
	cargo doc --open --no-deps

## --- Sandbox image builds (Approach B in docs/evalhub-redteam.md) ---
## Copies the musl binary into each image dir (gitignored there) and builds
## with `podman build -f`, since the CLI's `--from <dir>` mode only works
## for local gateways and requires a file literally named `Dockerfile`.

image-codex: musl ## Build the codex-garak sandbox image
	cp $(MUSL_BIN) $(CODEX_IMAGE_DIR)/agent-proxy
	podman build -t $(GARAK_IMAGE_REGISTRY)/codex-garak:latest \
	    -f $(CODEX_IMAGE_DIR)/Containerfile $(CODEX_IMAGE_DIR)/

image-claude: musl ## Build the claude-garak sandbox image
	cp $(MUSL_BIN) $(CLAUDE_IMAGE_DIR)/agent-proxy
	podman build -t $(GARAK_IMAGE_REGISTRY)/claude-garak:latest \
	    -f $(CLAUDE_IMAGE_DIR)/Containerfile $(CLAUDE_IMAGE_DIR)/

push-codex: ## Push the codex-garak image to $GARAK_IMAGE_REGISTRY
	podman push $(GARAK_IMAGE_REGISTRY)/codex-garak:latest

push-claude: ## Push the claude-garak image to $GARAK_IMAGE_REGISTRY
	podman push $(GARAK_IMAGE_REGISTRY)/claude-garak:latest

push: push-codex push-claude ## Push both sandbox images

## --- Release management with cargo-release ---
## Bumps Cargo.toml version, runs `make ci` as a gate (see release.toml),
## commits, tags agent-proxy-v<version>, and pushes. Never publishes to
## crates.io. GitHub Actions then builds the musl binary and both sandbox
## images and attaches/pushes them (see .github/workflows/release-agent-proxy.yml).

release-patch: ## Bump patch version (x.y.Z+1), tag, and push
	@echo "Creating patch release (x.y.Z+1)..."
	cargo release patch --execute

release-minor: ## Bump minor version (x.Y+1.0), tag, and push
	@echo "Creating minor release (x.Y+1.0)..."
	cargo release minor --execute

release-major: ## Bump major version (X+1.0.0), tag, and push
	@echo "Creating major release (X+1.0.0)..."
	cargo release major --execute
