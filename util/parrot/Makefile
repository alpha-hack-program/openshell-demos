# Pinned OpenShell SDK tag, mirrored in Cargo.toml's [workspace.dependencies].
# Override at the command line to repin: `make bump-openshell-sdk OPENSHELL_SDK_TAG=v0.0.116`
OPENSHELL_SDK_TAG ?= v0.0.106
OPENSHELL_REPO := https://github.com/NVIDIA/OpenShell

.PHONY: build fmt fmt-check clippy test check ci clean bump-openshell-sdk

# parrot-gpui is excluded from build/test (but not fmt/clippy, which don't
# link): it's an intentionally minimal, not-yet-functional stub, and
# linking it needs system X11 dev headers (libxcb, libxkbcommon,
# libxkbcommon-x11) that aren't guaranteed present in every dev/CI
# environment. `cargo check -p parrot-gpui` still verifies it type-checks.
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
