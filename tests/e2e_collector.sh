#!/usr/bin/env bash
# tests/e2e_collector.sh — E2E test for the Trail collector architecture.
#
# Phase 1 §1.11. The load-bearing proof that the collector + the SSH
# transport + the VPS-side plan-file append all work end-to-end against
# a real VPS reachable via SSH public-key auth.
#
# Run with:
#   bash tests/e2e_collector.sh
# Requires:
#   - VPS reachable over SSH (Tailscale, LAN, or public — set TRAIL_E2E_HOST)
#   - SSH key already in the ssh-agent / loadable from TRAIL_E2E_SSH_KEY
#   - Collector already built locally (see TRAIL_E2E_BINARY below)
#
# 7 steps:
#   1. install collector + schema + config on VPS in a temp dir
#   2. run -- `trail-collector health`  -> assert exit 0 + "ok": true
#   3. scp a test day-summary JSON into the VPS inbox
#   4. run -- `trail-collector once`     -> assert exit 0
#   5. verify the plan file <plan_root>/<YYYY-MM-DD>.md was appended
#   6. verify the source JSON moved to processed/
#   7. run -- `trail-collector validate` on a BAD file -> assert exit 1
#
# The script is PID-isolated via TEST_TAG so multiple parallel runs
# against the same VPS don't collide on temp-dir names. Trap-based
# cleanup (runs on success OR failure) removes the temp dir + any cron
# entry the script may have left behind.
#
# Skip mode: when TRAIL_E2E_HOST is unset OR --skip-ssh is passed, the
# script prints a SKIPPED message and exits 0. This makes the script a
# valid PR-able artifact even from hosts that can't reach the
# Tailscale-only VPS (e.g. the Linux build host). Re-run on the macOS
# laptop to exercise the real network path.

# Top-of-file SC2029 disable: every `ssh "${SSH_TARGET}" "<cmd>"` call below
# intentionally expands vars CLIENT-side before the body crosses the wire.
# That's the canonical pattern for one-shot e2e scripts — the plan-derived
# alternative (bash heredoc with `bash -s --`) is more correct for the
# *install* flow (which has to render paths on the server, see
# scripts/install-collector.sh §5b D1), but here the client KNOWS all the
# paths because IT generated `TEST_BASE` above, so client-side expansion
# is the simpler + more honest approach. Every value forwarded is a literal
# tmpdir path (no shell metacharacters), so the SC2029 risk is moot.
# shellcheck disable=SC2029

set -Eeuo pipefail

# ---- CLI args ------------------------------------------------------------

SKIP_SSH=0
for arg in "$@"; do
    case "$arg" in
        --skip-ssh) SKIP_SSH=1 ;;
        -h|--help)
            cat <<'USAGE'
Usage: bash tests/e2e_collector.sh [--skip-ssh]

Required environment (skip mode when unset):
  TRAIL_E2E_HOST       SSH target in user@host form (e.g. pedro@vm.x.y.z)
  TRAIL_E2E_BINARY     Local path to the trail-collector binary
                       (default: target/release/trail-collector)
  TRAIL_E2E_SSH_KEY    SSH private key path (default: ~/.ssh/id_ed25519)

When TRAIL_E2E_HOST is unset or --skip-ssh is passed, the script
prints "SKIPPED" and exits 0. Re-run on the macOS laptop to execute
the real network path against the VPS.
USAGE
            exit 0
            ;;
        *)
            echo "unknown arg: $arg" >&2
            exit 2
            ;;
    esac
done

# ---- env-var-driven config ----------------------------------------------

TRAIL_E2E_HOST="${TRAIL_E2E_HOST:-}"
TRAIL_E2E_BINARY="${TRAIL_E2E_BINARY:-target/release/trail-collector}"
TRAIL_E2E_SSH_KEY="${TRAIL_E2E_SSH_KEY:-${HOME}/.ssh/id_ed25519}"

# Skip mode: no VPS target configured, or explicit --skip-ssh flag.
if [[ -z "$TRAIL_E2E_HOST" || "$SKIP_SSH" -eq 1 ]]; then
    if [[ -z "$TRAIL_E2E_HOST" ]]; then
        echo "SKIPPED: TRAIL_E2E_HOST not set — re-run on the macOS laptop."
    else
        echo "SKIPPED: --skip-ssh flag set — re-run without the flag on the macOS laptop."
    fi
    echo "  host:      ${TRAIL_E2E_HOST:-<unset>}"
    echo "  binary:    ${TRAIL_E2E_BINARY:-target/release/trail-collector}"
    echo "  ssh key:   ${TRAIL_E2E_SSH_KEY:-${HOME}/.ssh/id_ed25519}"
    echo "  (this is a feature: the script is PR-able even from a host that can't reach Tailscale)"
    exit 0
fi

# ---- derive paths & identity -------------------------------------------

# Extract user@host parts so we can build the SSH target + the VPS
# home dir deterministically. We refuse anything that isn't in
# user@host form — bare hostnames would silently place the temp dir
# under a different user's home and cross-contaminate parallel runs.
if [[ "$TRAIL_E2E_HOST" != *@* ]]; then
    echo "error: TRAIL_E2E_HOST must be in user@host form, got: $TRAIL_E2E_HOST" >&2
    exit 2
fi
VPS_USER="${TRAIL_E2E_HOST%@*}"
VPS_HOSTNAME="${TRAIL_E2E_HOST#*@}"
if [[ -z "$VPS_USER" || -z "$VPS_HOSTNAME" ]]; then
    echo "error: TRAIL_E2E_HOST missing user or host part: $TRAIL_E2E_HOST" >&2
    exit 2
fi

TEST_TAG="trail-e2e-$$"  # PID-isolated; parallel runs don't collide

# Resolve the script's own location so the binary path can be relative
# to the repo root even when invoked from elsewhere.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# If TRAIL_E2E_BINARY is relative, anchor it to the repo root.
if [[ "$TRAIL_E2E_BINARY" != /* ]]; then
    TRAIL_E2E_BINARY="${REPO_ROOT}/${TRAIL_E2E_BINARY}"
fi

# VPS-side temp layout. All paths live under a single TEST_TAG dir so
# cleanup is a single `rm -rf`.
VPS_HOME="/home/${VPS_USER}"
TEST_BASE="${VPS_HOME}/.trail-e2e/${TEST_TAG}"
TEST_BIN="${TEST_BASE}/bin/trail-collector"
TEST_SCHEMA_DIR="${TEST_BASE}/schema"
TEST_INBOX="${TEST_BASE}/inbox"
TEST_PROCESSED="${TEST_BASE}/processed"
TEST_FAILED="${TEST_BASE}/failed"
TEST_PLAN_DIR="${TEST_BASE}/plans"
TEST_CONFIG="${TEST_BASE}/collector.json"
TEST_LOG="${TEST_BASE}/collector.log"
TEST_SCHEMA_FILE="${TEST_SCHEMA_DIR}/day-summary.schema.json"

# Local-side temp dirs (state files + fixtures the script generates
# before scp'ing them to the VPS).
LOCAL_TMP_DIR="$(mktemp -d -t "trail-e2e-XXXXXX")"
LOCAL_HEALTH_JSON="${LOCAL_TMP_DIR}/health.json"
LOCAL_PLAN_MD="${LOCAL_TMP_DIR}/plan.md"
LOCAL_VALIDATE_JSON="${LOCAL_TMP_DIR}/validate.json"

SSH_TARGET="${VPS_USER}@${VPS_HOSTNAME}"

# ssh options applied to every remote call: non-interactive (-T),
# batch (no host-key prompt), and the configured identity.
SSH_OPTS=(-T -o BatchMode=yes -o StrictHostKeyChecking=accept-new -i "${TRAIL_E2E_SSH_KEY}")

# ---- reachability probe --------------------------------------------------

# Fail fast with a clear message if the VPS is unreachable. The probe
# uses `true` so even a key with no shell access yields an honest exit.
echo "--- preflight: checking reachability of ${VPS_USER}@${VPS_HOSTNAME} ---"
if ! ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" "true" 2>/tmp/ssh-probe.err; then
    err_msg="$(cat /tmp/ssh-probe.err)"
    rm -f /tmp/ssh-probe.err
    echo
    echo "FATAL: VPS not reachable from this host (${VPS_USER}@${VPS_HOSTNAME})."
    echo
    echo "  ssh said:"
    # shellcheck disable=SC2001 # intentional: sed pipeline is the readable
    # form here; bash parameter expansion would re-wrap the indented lines.
    echo "${err_msg}" | sed 's/^/    /'
    echo
    echo "  The Phase 1 VPS is on a Tailscale network. This script must be"
    echo "  re-run on the macOS laptop where Tailscale is connected and the"
    echo "  SSH key is in the keychain."
    echo
    echo "  On the laptop:"
    echo "    tailscale status                                    # confirm VPS is online"
    echo "    ssh-add --apple-use-keychain ~/.ssh/id_ed25519     # load the key"
    echo "    TRAIL_E2E_HOST=pedro@<host> bash tests/e2e_collector.sh"
    echo
    echo "  Or to skip the network entirely:"
    echo "    bash tests/e2e_collector.sh --skip-ssh"
    echo
    rm -rf "${LOCAL_TMP_DIR}"
    exit 0  # Treat unreachable as "skip mode" so the script is still
            # CI-friendly; the operator can re-run on the laptop.
fi
rm -f /tmp/ssh-probe.err

# ---- pre-flight checks (local) -------------------------------------------

if [[ ! -x "${TRAIL_E2E_BINARY}" && ! -f "${TRAIL_E2E_BINARY}" ]]; then
    echo "FATAL: collector not built at ${TRAIL_E2E_BINARY}" >&2
    echo "Build with: cargo build --release -p trail-collector" >&2
    echo "Or pass TRAIL_E2E_BINARY env var to point at an alternative build." >&2
    cleanup
    exit 1
fi

LOCAL_SCHEMA="${REPO_ROOT}/resources/day-summary.schema.json"
if [[ ! -f "${LOCAL_SCHEMA}" ]]; then
    echo "FATAL: schema not bundled at ${LOCAL_SCHEMA}" >&2
    echo "The repo root should contain resources/day-summary.schema.json" >&2
    cleanup
    exit 1
fi

# ---- trap-based cleanup -------------------------------------------------

cleanup() {
    local rc=$?
    # Only print the cleanup banner when this is the final cleanup
    # pass (rc != 0 indicates we failed mid-run; 0 means we got past
    # the success line). Avoids duplicate output when re-entering
    # cleanup during the pre-flight checks above.
    if [[ $rc -ne 0 ]]; then
        echo
        echo "=== E2E FAILED (exit ${rc}) — cleanup running ==="
    fi
    # Remote cleanup is best-effort; never let cleanup itself fail
    # the exit status.
    ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" \
        "rm -rf '${TEST_BASE}' && (crontab -l 2>/dev/null | grep -v -F '${TEST_TAG}' || true) | crontab -" \
        >/dev/null 2>&1 || true
    rm -rf "${LOCAL_TMP_DIR}"
}
trap cleanup EXIT
on_err() {
    local rc=$?
    local line="${BASH_LINENO[0]}"
    echo "=== E2E FAILED at line ${line} (exit ${rc}) ===" >&2
    exit "${rc}"
}
trap on_err ERR

# ---- step 1: install collector + schema + config on VPS -----------------

echo
echo "--- 1. install collector + schema + config on VPS ---"

ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" \
    "mkdir -p '${TEST_BASE}/bin' '${TEST_SCHEMA_DIR}' '${TEST_INBOX}' \
             '${TEST_PROCESSED}' '${TEST_FAILED}' '${TEST_PLAN_DIR}'"

scp -i "${TRAIL_E2E_SSH_KEY}" "${TRAIL_E2E_BINARY}" "${SSH_TARGET}:${TEST_BIN}"
scp -i "${TRAIL_E2E_SSH_KEY}" "${LOCAL_SCHEMA}"     "${SSH_TARGET}:${TEST_SCHEMA_FILE}"
ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" "chmod 0755 '${TEST_BIN}' && chmod 0644 '${TEST_SCHEMA_FILE}' '${TEST_CONFIG}'" || true

# Render collector.json on the laptop (no secrets, just paths under
# the temp dir). The server never has to expand any of these — they
# are absolute paths and `$` characters don't appear inside.
cat > "${LOCAL_TMP_DIR}/collector.json" <<EOF
{
  "inbox_dir":         "${TEST_INBOX}",
  "processed_dir":     "${TEST_PROCESSED}",
  "failed_dir":        "${TEST_FAILED}",
  "plan_root":         "${TEST_PLAN_DIR}",
  "plan_template":     "{date}.md",
  "schema_path":       "${TEST_SCHEMA_FILE}",
  "log_path":          "${TEST_LOG}",
  "user":              "${VPS_USER}",
  "schema_validation": "strict"
}
EOF
scp -i "${TRAIL_E2E_SSH_KEY}" "${LOCAL_TMP_DIR}/collector.json" "${SSH_TARGET}:${TEST_CONFIG}"
ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" "chmod 0644 '${TEST_CONFIG}'"

echo "  bin:     ${TEST_BIN}"
echo "  schema:  ${TEST_SCHEMA_FILE}"
echo "  config:  ${TEST_CONFIG}"

# ---- step 2: run -- `trail-collector health` ----------------------------

echo
echo "--- 2. run --health (assert exit 0 + JSON ok:true) ---"

ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" \
    "'${TEST_BIN}' --config '${TEST_CONFIG}' health" \
    > "${LOCAL_HEALTH_JSON}"

# Print what the server returned so the operator can read it from the
# verification log.
echo "  --health stdout:"
sed 's/^/    /' "${LOCAL_HEALTH_JSON}"

# `--health` exits 0 on success; the JSON payload has "ok": true.
# We grep for the literal because the JSON is small + ordered.
if ! grep -q '"ok": true' "${LOCAL_HEALTH_JSON}"; then
    echo "FATAL: --health did not return ok=true" >&2
    exit 1
fi

# ---- step 3: push a test day-summary into the inbox --------------------

echo
echo "--- 3. push a test day-summary JSON to the VPS inbox ---"

TEST_DATE="$(date -u +%Y-%m-%d)"
cat > "${LOCAL_TMP_DIR}/${TEST_DATE}.json" <<EOF
{
  "date":         "${TEST_DATE}",
  "summary":      "E2E test from tests/e2e_collector.sh (tag=${TEST_TAG})",
  "wins":         ["e2e test green", "architecture proven"],
  "blockers":     [],
  "people":       ["e2e-bot"],
  "open_threads": ["next: phase 2"],
  "voice_notes":  []
}
EOF
scp -i "${TRAIL_E2E_SSH_KEY}" "${LOCAL_TMP_DIR}/${TEST_DATE}.json" \
    "${SSH_TARGET}:${TEST_INBOX}/${TEST_DATE}.json"
echo "  inboxed: ${TEST_INBOX}/${TEST_DATE}.json"

# ---- step 4: run -- `trail-collector once` -----------------------------

echo
echo "--- 4. run --once (assert exit 0) ---"

# `--once` processes the inbox. Exit 0 = clean (could be empty inbox,
# but in our case there's exactly one file). Exit 2 = at least one
# file errored (we want clean exit for the happy-path proof).
ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" \
    "'${TEST_BIN}' --config '${TEST_CONFIG}' once"

# ---- step 5: verify the plan file was appended -------------------------

echo
echo "--- 5. verify the plan file was appended ---"

scp -i "${TRAIL_E2E_SSH_KEY}" "${SSH_TARGET}:${TEST_PLAN_DIR}/${TEST_DATE}.md" \
    "${LOCAL_PLAN_MD}"

echo "  plan file contents:"
sed 's/^/    /' "${LOCAL_PLAN_MD}"

# The day-summary section should contain the tag + the wins content.
if ! grep -q "tests/e2e_collector.sh" "${LOCAL_PLAN_MD}"; then
    echo "FATAL: plan file does not contain the e2e tag marker" >&2
    exit 1
fi
if ! grep -q "e2e test green" "${LOCAL_PLAN_MD}"; then
    echo "FATAL: plan file does not contain the wins content" >&2
    exit 1
fi

# ---- step 6: verify the source JSON moved to processed/ ----------------

echo
echo "--- 6. verify the source JSON moved to processed/ ---"

if ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" "test -f '${TEST_INBOX}/${TEST_DATE}.json'"; then
    echo "FATAL: file still in inbox after --once" >&2
    exit 1
fi
if ! ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" "test -f '${TEST_PROCESSED}/${TEST_DATE}.json'"; then
    echo "FATAL: file not in processed/ after --once" >&2
    exit 1
fi
echo "  file is no longer in inbox; processed/${TEST_DATE}.json present"

# ---- step 7: run -- `trail-collector validate` on a BAD file -----------

echo
echo "--- 7. run --validate on a BAD file (assert exit 1 + ok:false) ---"

echo '{ "not": "a day summary" }' > "${LOCAL_TMP_DIR}/bad.json"
scp -i "${TRAIL_E2E_SSH_KEY}" "${LOCAL_TMP_DIR}/bad.json" \
    "${SSH_TARGET}:${TEST_INBOX}/bad.json"

# `--validate` is the explicit anti-check: a real bug would be
# `--validate` accepting this file. Exit 1 is the contract (see
# crates/trail-collector/src/validate.rs).
set +e
ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" \
    "'${TEST_BIN}' --config '${TEST_CONFIG}' validate '${TEST_INBOX}/bad.json'" \
    > "${LOCAL_VALIDATE_JSON}" 2>&1
validate_exit=$?
set -e

echo "  --validate exit code: ${validate_exit}"
echo "  --validate stdout/stderr:"
sed 's/^/    /' "${LOCAL_VALIDATE_JSON}"

if [[ "${validate_exit}" -eq 0 ]]; then
    echo "FATAL: --validate accepted a bad file (exit 0)" >&2
    exit 1
fi
if ! grep -q '"ok": false' "${LOCAL_VALIDATE_JSON}"; then
    echo "FATAL: --validate did not return ok:false in the JSON payload" >&2
    exit 1
fi

# Best-effort cleanup of the bad.json we left in the inbox (the
# overall `cleanup` trap will rm -rf the entire TEST_BASE anyway on
# exit, so this is just defensive in case the script hangs later).
ssh "${SSH_OPTS[@]}" "${SSH_TARGET}" "rm -f '${TEST_INBOX}/bad.json'" >/dev/null 2>&1 || true

# ---- success ------------------------------------------------------------

# Override the cleanup trap for the success path so the success
# banner prints first; cleanup still runs.
echo
echo "=== E2E PASSED ==="
echo "  test tag:    ${TEST_TAG}"
echo "  date:        ${TEST_DATE}"
echo "  vps user:    ${VPS_USER}"
echo "  vps host:    ${VPS_HOSTNAME}"
echo "  test base:   ${TEST_BASE}"
echo "  artifacts:"
echo "    health output:    ${LOCAL_HEALTH_JSON}"
echo "    plan output:      ${LOCAL_PLAN_MD}"
echo "    validate output:  ${LOCAL_VALIDATE_JSON}"
