# Trail — top-level developer Makefile
#
# Convenience targets for the most common commands. All targets are
# thin wrappers around the underlying tooling (cargo, pnpm, tauri-cli)
# so contributors can stay in their usual `make <goal>` workflow
# without typing the full `cargo test --workspace && pnpm test` line.
#
# Conventions:
#   - `make help` (default target) prints what targets exist.
#   - Phony targets are the rule; nothing in this Makefile produces
#     a file named `test`, `build`, etc.
#   - `make install-collector` is the standalone binary install path
#     documented in §7.6 of the Phase 7 plan. It runs
#     `cargo install --path crates/trail-collector --locked` so the
#     lockfile is honored (no surprise upgrades).

SHELL := /bin/bash

# Stable rust toolchain (matches rust-toolchain.toml at the repo root).
RUST ?= cargo
TAURI ?= pnpm tauri

.DEFAULT_GOAL := help

.PHONY: help install-collector dev build test lint fmt fmt-check clean

help: ## show this help
	@echo "Trail — common developer targets:"
	@echo
	@grep -E '^[a-zA-Z_-]+:.*?## .*$$' $(MAKEFILE_LIST) | \
	    awk 'BEGIN {FS = ":.*?## "}; {printf "  \033[36m%-22s\033[0m %s\n", $$1, $$2}'
	@echo
	@echo "Run 'make <target>' to invoke."

install-collector: ## install trail-collector from local checkout to ~/.cargo/bin
	$(RUST) install --path crates/trail-collector --locked

dev: ## run the Tauri app in development mode (requires a display)
	$(TAURI) dev

build: ## build the Tauri app (signed .app + .dmg on macOS; .deb/.AppImage on Linux)
	$(TAURI) build

test: ## run all workspace tests (Rust + frontend)
	$(RUST) test --workspace
	$(TAURI) test

lint: ## fmt-check + clippy with -D warnings + frontend lint
	$(RUST) fmt --check
	$(RUST) clippy --workspace --all-targets -- -D warnings
	$(TAURI) lint

fmt: ## format Rust sources in-place
	$(RUST) fmt --all

fmt-check: ## verify Rust formatting without modifying files
	$(RUST) fmt --check

clean: ## remove build artifacts (target/, node_modules/dist)
	$(RUST) clean
	rm -rf node_modules/.cache
