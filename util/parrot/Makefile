# Pinned OpenShell SDK tag, mirrored in Cargo.toml's [workspace.dependencies].
# Override at the command line to repin: `make bump-openshell-sdk OPENSHELL_SDK_TAG=v0.0.116`
OPENSHELL_SDK_TAG ?= v0.0.106
OPENSHELL_REPO := https://github.com/NVIDIA/OpenShell

.PHONY: build fmt fmt-check clippy test check ci clean bump-openshell-sdk \
        release-patch release-minor release-major

# parrot-gpui is a real, functional frontend, but stays out of build/test
# (unlike fmt/clippy, which don't link): linking it needs system X11 dev
# headers (libxcb, libxkbcommon, libxkbcommon-x11) that aren't guaranteed
# present in every dev environment. CI installs them explicitly and builds
# it separately (see .github/workflows/ci-parrot.yml). `cargo check -p
# parrot-gpui` still verifies it type-checks without those headers.
build:
	cargo build -p parrot-core -p parrot-tui

fmt:
	cargo fmt --all

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --workspace --all-targets -- -D warnings

test:
	cargo test -p parrot-core -p parrot-tui

check: fmt-check clippy test ## Run fmt-check, clippy, and tests

ci: check ## Alias for check, matches CI gate

clean:
	cargo clean

# Repin openshell-sdk / openshell-bootstrap to a different tag, branch, or
# commit (anything `git = "...", tag = "..."` accepts) and refresh the lock.
bump-openshell-sdk:
	sed -i -E 's#(openshell-(sdk|bootstrap) = \{ git = "$(OPENSHELL_REPO)", tag = )"[^"]+"#\1"$(OPENSHELL_SDK_TAG)"#' Cargo.toml
	cargo update -p openshell-sdk -p openshell-bootstrap
	@echo "Pinned openshell-sdk / openshell-bootstrap to $(OPENSHELL_SDK_TAG)"

## --- Release management with cargo-release ---
## Bumps the shared workspace version (all 3 crates inherit it), runs
## `make ci` as a gate (see release.toml), commits once, tags
## parrot-v<version> (one tag for the whole workspace, not per-crate —
## release.toml pins tag-name explicitly since the default template would
## otherwise produce parrot-core-v/parrot-tui-v/parrot-gpui-v separately),
## and pushes. Never publishes to crates.io. Requires a clean working tree
## across the whole repo, not just util/parrot/.

release-patch: ## Bump patch version (x.y.Z+1), tag, and push
	@echo "Creating patch release (x.y.Z+1)..."
	cargo release patch --workspace --execute

release-minor: ## Bump minor version (x.Y+1.0), tag, and push
	@echo "Creating minor release (x.Y+1.0)..."
	cargo release minor --workspace --execute

release-major: ## Bump major version (X+1.0.0), tag, and push
	@echo "Creating major release (X+1.0.0)..."
	cargo release major --workspace --execute
