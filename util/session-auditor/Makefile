.DEFAULT_GOAL := help

BIN            := session-auditor
MUSL_TARGET    := x86_64-unknown-linux-musl
MUSL_BIN       := target/$(MUSL_TARGET)/release/$(BIN)
IMAGE_REGISTRY ?= quay.io/atarazana

# One image per agent — same binary, different base image + hook-config
# file (managed-settings.json for Claude Code, requirements.toml for
# Codex). The image's own tag is the *base* image's tag, verbatim, so a
# base-image bump is visible rather than silently going stale — see
# util/session-auditor/README.md.
CLAUDE_IMAGE_DIR := ../../demos/keycloak-oidc/images/claude-audit
CODEX_IMAGE_DIR  := ../../demos/keycloak-oidc/images/codex-audit
CLAUDE_BASE_TAG  := $(lastword $(subst :, ,$(shell grep '^FROM' $(CLAUDE_IMAGE_DIR)/Containerfile)))
CODEX_BASE_TAG   := $(lastword $(subst :, ,$(shell grep '^FROM' $(CODEX_IMAGE_DIR)/Containerfile)))

.PHONY: help build release musl \
        fmt fmt-check clippy check ci \
        clean doc \
        image-claude image-codex push-claude push-codex push \
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

## --- Sandbox image builds, one per agent ---
## Bakes the musl binary into the stock agent sandbox image at
## /usr/local/bin/session-auditor — this is load-bearing, not a convenience:
## /usr (and therefore /usr/local/bin) is read_only under OpenShell's
## default filesystem_policy, enforced via Landlock, and `sandbox upload`
## cannot write there once the sandbox exists. Baking the binary in here,
## before the sandbox is created, is the only way to get a copy of it into
## a path nothing inside a running sandbox can replace. See
## demos/keycloak-oidc/docs/prometheus-scraping.md.
## Copies the musl binary into the image dir (gitignored there) and builds
## with `podman build -f`, since the CLI's `--from <dir>` mode only works
## for local gateways and requires a file literally named `Dockerfile`.
## Each image is tagged with its *base* image's own tag (extracted from
## the Containerfile's FROM line) plus `latest` — not this crate's own
## version — so the tag itself answers "which base image is this built
## against."

image-claude: musl ## Build the claude-audit sandbox image
	cp $(MUSL_BIN) $(CLAUDE_IMAGE_DIR)/session-auditor
	podman build \
	    -t $(IMAGE_REGISTRY)/claude-audit:$(CLAUDE_BASE_TAG) \
	    -t $(IMAGE_REGISTRY)/claude-audit:latest \
	    -f $(CLAUDE_IMAGE_DIR)/Containerfile $(CLAUDE_IMAGE_DIR)/

image-codex: musl ## Build the codex-audit sandbox image
	cp $(MUSL_BIN) $(CODEX_IMAGE_DIR)/session-auditor
	podman build \
	    -t $(IMAGE_REGISTRY)/codex-audit:$(CODEX_BASE_TAG) \
	    -t $(IMAGE_REGISTRY)/codex-audit:latest \
	    -f $(CODEX_IMAGE_DIR)/Containerfile $(CODEX_IMAGE_DIR)/

push-claude: ## Push the claude-audit image to $IMAGE_REGISTRY
	podman push $(IMAGE_REGISTRY)/claude-audit:$(CLAUDE_BASE_TAG)
	podman push $(IMAGE_REGISTRY)/claude-audit:latest

push-codex: ## Push the codex-audit image to $IMAGE_REGISTRY
	podman push $(IMAGE_REGISTRY)/codex-audit:$(CODEX_BASE_TAG)
	podman push $(IMAGE_REGISTRY)/codex-audit:latest

push: push-claude push-codex ## Push both sandbox images

## --- Release management with cargo-release ---
## Bumps Cargo.toml version, runs `make ci` as a gate (see release.toml),
## commits, tags session-auditor-v<version>, and pushes. Never publishes to
## crates.io. GitHub Actions then builds the musl binary and the sandbox
## image and attaches/pushes them (see .github/workflows/release-session-auditor.yml).

release-patch: ## Bump patch version (x.y.Z+1), tag, and push
	@echo "Creating patch release (x.y.Z+1)..."
	cargo release patch --execute

release-minor: ## Bump minor version (x.Y+1.0), tag, and push
	@echo "Creating minor release (x.Y+1.0)..."
	cargo release minor --execute

release-major: ## Bump major version (X+1.0.0), tag, and push
	@echo "Creating major release (X+1.0.0)..."
	cargo release major --execute
