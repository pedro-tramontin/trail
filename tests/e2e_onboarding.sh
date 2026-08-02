#!/usr/bin/env bash
# tests/e2e_onboarding.sh — Headless end-to-end smoke for the
# Phase 6 onboarding harness.
#
# Wraps the same Phase A → B → C → D flow exercised by
# `src-tauri/tests/onboarding_e2e.rs` as a shell-friendly smoke
# check. Drives:
#
#   1. `cargo build -p mock-ssh-server` (preflight for Phase D).
#   2. `cargo test -p trail --test onboarding_e2e` (the actual
#      Phase A → B → C → D walk; spawns mock-ssh-server internally
#      against the wiremocked ollama + temp config).
#
# Skip mode: when `TRAIL_E2E_HOST` is unset the script prints a
# SKIPPED banner and exits 0. Same env-var convention as
# `tests/e2e_voice.sh` (Phase 5), `tests/e2e_collector.sh`
# (Phase 1), and `tests/e2e_collectors.sh` (Phase 2) — the script
# is PR-able from a Linux build host without a real VPS / live
# ollama / wizard wizard UI.
#
# Override behaviour (advanced; mostly for CI):
#   TRAIL_E2E_HOST=1 bash tests/e2e_onboarding.sh   → run the full test
#   TRAIL_E2E_HOST=0 bash tests/e2e_onboarding.sh   → forced skip
#   --skip-host                                     → same as TRAIL_E2E_HOST=0

set -Eeuo pipefail

# ---- toolchain bootstrapping --------------------------------------------
# `cargo` may resolve through rustup on the orchestrator host; the
# default `stable` toolchain is what the rest of the workspace pins.
export RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}"
# Prefer the system-installed stable toolchain over a stale
# `rustup default` resolution when one is on PATH.
if [[ -x "/root/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin/cargo" ]]; then
    export PATH="/root/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/bin:${PATH}"
fi

# ---- CLI args ------------------------------------------------------------

SKIP_HOST=0
for arg in "$@"; do
    case "$arg" in
        --skip-host) SKIP_HOST=1 ;;
        -h|--help)
            cat <<'USAGE'
Usage: bash tests/e2e_onboarding.sh [--skip-host]

Required environment (skip mode when unset):
  TRAIL_E2E_HOST     Any non-empty value triggers the full
                     e2e. The variable contents are not
                     inspected — only the "is it set?" check
                     matters. Acts as the opt-in signal that a
                     real e2e run should execute (i.e. the host
                     has the orchestrator's preflight toolchain
                     and a writable /tmp).

When TRAIL_E2E_HOST is unset OR --skip-host is passed, the
script prints "SKIPPED" and exits 0. This makes the script
PR-able from a headless Linux build host.

The full run executes the Rust integration test
src-tauri/tests/onboarding_e2e.rs (4 cases) which walks the
Phase A → B → C → D pipeline against fixture filesystem state,
wiremocked ollama, and the in-tree mock-ssh-server fixture.
USAGE
            exit 0 ;;
        *)
            echo "unknown arg: $arg" >&2
            exit 2 ;;
    esac
done

# ---- env-var-driven config ----------------------------------------------

TRAIL_E2E_HOST="${TRAIL_E2E_HOST:-}"

# Skip mode: no host trigger, or explicit --skip-host flag. Same
# convention as tests/e2e_voice.sh / e2e_collector.sh / e2e_collectors.sh.
if [[ -z "$TRAIL_E2E_HOST" || "$SKIP_HOST" -eq 1 ]]; then
    if [[ "$SKIP_HOST" -eq 1 ]]; then
        echo "SKIPPED: --skip-host flag set — re-run without the flag on the orchestrator host."
    else
        echo "SKIPPED: TRAIL_E2E_HOST not set — re-run with TRAIL_E2E_HOST=1 to execute the integration test."
    fi
    echo "  host trigger:    ${TRAIL_E2E_HOST:-<unset>}"
    echo "  preflight:       cargo build -p mock-ssh-server"
    echo "  integration test:cargo test -p trail --test onboarding_e2e"
    echo "  (this is a feature: the script is PR-able from a headless Linux build host)"
    echo
    echo "=== E2E SKIPPED ==="
    exit 0
fi

# ---- derive paths --------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

echo
echo "=== Phase 6 e2e (TRAIL_E2E_HOST=${TRAIL_E2E_HOST}) ==="
echo "  repo root: ${REPO_ROOT}"
echo

# ---- preflight: build the mock-ssh-server binary ------------------------

echo
echo "--- 1. cargo build -p mock-ssh-server (Phase D preflight) ---"

(
    cd "${REPO_ROOT}" && \
        cargo build -p mock-ssh-server 2>&1 | tail -5
)

# ---- run the Rust integration test -------------------------------------

echo
echo "--- 2. cargo test -p trail --test onboarding_e2e (Phase A → B → C → D) ---"

(
    cd "${REPO_ROOT}" && \
        cargo test -p trail --test onboarding_e2e 2>&1 | tail -20
)

echo
echo "=== Phase 6 e2e PASSED ==="
