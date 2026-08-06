.DEFAULT_GOAL := help

BIN          := onboard
BIN_PATH     := target/release/$(BIN)

.PHONY: help build release install run watch test test-verbose \
        fmt fmt-check clippy check ci \
        clean doc \
        release-patch release-minor release-major

help: ## Show this help
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | sort | \
		awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}'

build: ## Debug build
	cargo build

release: ## Release build
	cargo build --release

install: release ## Build release and install to ~/bin
	install -d $(HOME)/bin
	install -m 755 $(BIN_PATH) $(HOME)/bin/$(BIN)

run: ## Run in debug mode, e.g. make run ARGS="--help"
	cargo run -- $(ARGS)

watch: ## Rebuild and rerun on file changes (requires cargo-watch)
	cargo watch -x 'run'

test: ## Run the test suite
	cargo test

test-verbose: ## Run tests with output shown
	cargo test -- --nocapture

fmt: ## Format the code
	cargo fmt

fmt-check: ## Check formatting without modifying files
	cargo fmt --check

clippy: ## Lint with clippy, warnings as errors
	cargo clippy --all-targets -- -D warnings

check: fmt-check clippy test ## Run fmt-check, clippy, and tests

ci: check ## Alias for check, matches CI gate

clean: ## Remove build artifacts
	cargo clean

doc: ## Build and open docs
	cargo doc --open --no-deps

## --- Release management with cargo-release ---
## Bumps Cargo.toml version, runs `make ci` as a gate (see release.toml),
## commits, tags v<version>, and pushes. Never publishes to crates.io.

release-patch: ## Bump patch version (x.y.Z+1), tag, and push
	@echo "Creating patch release (x.y.Z+1)..."
	cargo release patch --execute

release-minor: ## Bump minor version (x.Y+1.0), tag, and push
	@echo "Creating minor release (x.Y+1.0)..."
	cargo release minor --execute

release-major: ## Bump major version (X+1.0.0), tag, and push
	@echo "Creating major release (X+1.0.0)..."
	cargo release major --execute
