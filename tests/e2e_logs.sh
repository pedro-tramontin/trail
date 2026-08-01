#!/usr/bin/env bash
# Phase 4 §4.5 logs backend e2e.
#
# Wraps the Cargo integration test `src-tauri/tests/e2e_logs.rs`,
# which exercises the pure-function entry points (list_logs,
# delete_log, get_raw_json) against the real
# `tests/fixtures/raw/2026-07-29/*.json` fixtures.
#
# We deliberately do NOT exercise Tauri IPC here — that's verified
# on Pedro's Mac. This harness proves the backend logic + the
# fixture contract.
#
# TEST_TAG is propagated into the log output so a CI run for this
# harness can be grep'd out of a combined log stream — see the
# matching `trail-e2e-summarizer-*` pattern in tests/e2e_summarizer.sh.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_TAG="trail-e2e-logs-$$"

echo "[${TEST_TAG}] === PHASE 4 LOGS E2E ==="
echo "[${TEST_TAG}] [1/2] Running cargo integration test..."
(cd "$REPO_ROOT/src-tauri" && cargo test --test e2e_logs -- --nocapture)
echo "[${TEST_TAG}] [2/2] Running full Rust regression..."
(cd "$REPO_ROOT" && cargo test -p trail --quiet)
echo "[${TEST_TAG}] === PHASE 4 LOGS E2E PASSED ==="