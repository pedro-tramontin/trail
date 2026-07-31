#!/usr/bin/env bash
# tests/e2e_collectors.sh — End-to-end test for the Phase 2 laptop-side
# collectors (github, claude-sessions, calendar).
#
# Hermetic on a Linux build host: uses a stub `gh` in PATH (no real GitHub
# network call) and the bundled on-disk fixtures (no real `gh` auth, no real
# macOS `.ics`). The supervisor roundtrip — `--collect <source> --laptop-config
# <file>` → schema validation → write to `~/.trail/raw/<date>/<source>.json`
# → re-validate on disk — is exercised for all three sources.
#
# Run:
#   bash tests/e2e_collectors.sh
# Requires:
#   - collector built at $TRAIL_E2E_BINARY (default: target/release/trail-collector)
#   - the per-source JSON Schemas at crates/trail-collector/schemas/*.json
#     AND bundled into src-tauri/resources/*.json by a prior `cargo build`.
#
# Skip mode: when TRAIL_E2E_LAPTOP_CONFIG is unset the script prints a
# SKIPPED banner and exits 0. This makes the script PR-able from a host
# where the real `gh` auth and the real macOS `.ics` are not available
# (e.g. the Linux CI / build host). On the macOS laptop, exporting
# TRAIL_E2E_LAPTOP_CONFIG to any non-empty value enables the full run.
# Real-platform smoke (where `gh` is auth'd and the `.ics` is the
# user's actual Apple Calendar export) is still Pedro's Mac verification
# per the Phase 2 plan's "Headless-environment degradation" section.

set -Eeuo pipefail

# ---- CLI args ------------------------------------------------------------

SKIP=0
for arg in "$@"; do
    case "$arg" in
        --skip)         SKIP=1 ;;
        -h|--help)
            cat <<'USAGE'
Usage: bash tests/e2e_collectors.sh [--skip]

Required environment (skip mode when unset):
  TRAIL_E2E_LAPTOP_CONFIG  Any non-empty value. The script does not read
                           the contents — it only checks the variable is
                           set. Per-source fixture data is generated
                           in-tree by the script.
  TRAIL_E2E_BINARY         Local path to the trail-collector binary
                           (default: target/release/trail-collector)

When TRAIL_E2E_LAPTOP_CONFIG is unset OR --skip is passed, the script
prints "SKIPPED" and exits 0. The script is hermetic: it uses a stub
`gh` in PATH, an inline .ics fixture, and the in-tree
claude_sessions JSONL fixtures, so it runs on any Linux box with the
collector binary present.
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

TRAIL_E2E_LAPTOP_CONFIG="${TRAIL_E2E_LAPTOP_CONFIG:-}"
TRAIL_E2E_BINARY="${TRAIL_E2E_BINARY:-target/release/trail-collector}"

# Skip mode: no laptop-config trigger, or explicit --skip flag. Same
# convention as tests/e2e_collector.sh (Phase 1) — PR-able from any host.
if [[ -z "$TRAIL_E2E_LAPTOP_CONFIG" || "$SKIP" -eq 1 ]]; then
    if [[ "$SKIP" -eq 1 ]]; then
        echo "SKIPPED: --skip flag set — re-run without the flag to execute the script."
    else
        echo "SKIPPED: TRAIL_E2E_LAPTOP_CONFIG not set — re-run on the macOS laptop."
    fi
    echo "  laptop-config trigger: ${TRAIL_E2E_LAPTOP_CONFIG:-<unset>}"
    echo "  binary:                ${TRAIL_E2E_BINARY}"
    echo "  (this is a feature: the script is PR-able even from a host without real gh auth or a real macOS .ics)"
    exit 0
fi

# ---- derive paths -------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# If TRAIL_E2E_BINARY is relative, anchor it to the repo root.
if [[ "$TRAIL_E2E_BINARY" != /* ]]; then
    TRAIL_E2E_BINARY="${REPO_ROOT}/${TRAIL_E2E_BINARY}"
fi

# The supervisor reads the per-source schema from `cfg.schema_path` (the
# caller-supplied absolute path inside the laptop config). The plan says
# to prefer the BUNDLED copy under src-tauri/resources/ because that is
# what the Tauri app ships at runtime — the crates/-side copy is the
# source-of-truth edit location, but the Tauri app reads from
# src-tauri/resources/. Re-validation against the bundled copy catches
# any drift between the two. The collector itself also reads from
# `cfg.schema_path`, so we point both at the bundled copy.
RESOURCES="${REPO_ROOT}/src-tauri/resources"
SCHEMA_ROOT="${REPO_ROOT}/crates/trail-collector/schemas"

# Per-source fixtures (in-tree; no network, no real .ics, no real gh).
FIXTURE_GH_SEARCH="${REPO_ROOT}/crates/trail-collector/tests/fixtures/github/gh_search_author.json"
FIXTURE_GH_VIEW="${REPO_ROOT}/crates/trail-collector/tests/fixtures/github/gh_prs_view.json"
FIXTURE_GH_COMMITS="${REPO_ROOT}/crates/trail-collector/tests/fixtures/github/gh_prs_commits.json"
FIXTURE_CAL="${REPO_ROOT}/crates/trail-collector/tests/fixtures/calendar/workday.ics"
FIXTURE_CLAUDE_DIR="$(dirname "${REPO_ROOT}/crates/trail-collector/tests/fixtures/claude_sessions/sessions.jsonl")/.."
# Synth collector expects "the configured path" to be a parent dir; the
# walk + privacy check run against every file under it. We point at the
# in-tree fixture dir directly — no copying, no rewriting. (The
# fixtures are not under `.local`, so the privacy guard is a no-op.)

# ---- pre-flight checks (local) ------------------------------------------

if [[ ! -x "${TRAIL_E2E_BINARY}" && ! -f "${TRAIL_E2E_BINARY}" ]]; then
    echo "FATAL: collector not built at ${TRAIL_E2E_BINARY}" >&2
    echo "Build with: cargo build --release -p trail-collector" >&2
    echo "Or pass TRAIL_E2E_BINARY env var to point at an alternative build." >&2
    exit 1
fi

for s in github claude_sessions calendar; do
    for root in "${SCHEMA_ROOT}" "${RESOURCES}"; do
        if [[ ! -f "${root}/${s}.schema.json" ]]; then
            echo "FATAL: missing schema ${root}/${s}.schema.json" >&2
            echo "Run a full \`cargo build --workspace\` first so the build.rs copies the schemas." >&2
            exit 1
        fi
    done
done

for f in "${FIXTURE_GH_SEARCH}" "${FIXTURE_GH_VIEW}" "${FIXTURE_GH_COMMITS}" \
         "${FIXTURE_CAL}" \
         "${REPO_ROOT}/crates/trail-collector/tests/fixtures/claude_sessions/sessions.jsonl"; do
    if [[ ! -f "$f" ]]; then
        echo "FATAL: missing fixture $f" >&2
        exit 1
    fi
done

# python3 is what the per-source re-validators use to round-trip the
# written JSON back through jsonschema. We try the in-tree binary
# first (some images only ship `python`), then fall back to system
# python3.
PY="${PY:-}"
if [[ -z "${PY}" ]]; then
    if command -v python3 >/dev/null 2>&1; then
        PY="$(command -v python3)"
    elif command -v python >/dev/null 2>&1; then
        PY="$(command -v python)"
    else
        echo "FATAL: neither python3 nor python found on PATH" >&2
        exit 1
    fi
fi

# Best-effort: install jsonschema for the python re-validators. If pip
# is not available or the install fails, the re-validators fall back to
# a structural sanity check (presence of the four required root keys).
if ! "${PY}" -c "import jsonschema" 2>/dev/null; then
    echo "--- preflight: installing python jsonschema (best-effort) ---"
    if command -v pip3 >/dev/null 2>&1; then
        pip3 install --quiet --user jsonschema 2>/dev/null || true
    elif command -v pip >/dev/null 2>&1; then
        pip install --quiet --user jsonschema 2>/dev/null || true
    fi
fi
if "${PY}" -c "import jsonschema" 2>/dev/null; then
    echo "  jsonschema: present (real validation enabled)"
    HAVE_JSONSCHEMA=1
else
    echo "  jsonschema: ABSENT (re-validators will use structural sanity only)"
    HAVE_JSONSCHEMA=0
fi

# ---- per-run temp dir + trap-based cleanup -----------------------------

TEST_TAG="trail-e2e-collectors-$$"  # PID-isolated; parallel runs don't collide
TEST_BASE="$(mktemp -d -t "trail-e2e-collectors-XXXXXX")"
STUB_DIR="${TEST_BASE}/gh-stub"
RAW_ROOT="${TEST_BASE}/raw"
# TODAY must match what the supervisor writes: `Local::now().date_naive()`
# (the per-source collectors use the same local-time date for the per-day
# directory name). On a UTC-build host this equals `date -u`; on a host
# in a non-UTC TZ it differs — use the local-time form so the script
# finds the files regardless of the host's timezone.
TODAY="$(date +%Y-%m-%d)"
DAY_DIR="${RAW_ROOT}/${TODAY}"

cleanup() {
    local rc=$?
    if [[ $rc -ne 0 ]]; then
        echo
        echo "=== E2E FAILED (exit ${rc}) — preserving ${TEST_BASE} for inspection ==="
    fi
    # Unwind PATH override (so the rest of the shell session is clean).
    if [[ -n "${ORIGINAL_PATH:-}" ]]; then
        export PATH="${ORIGINAL_PATH}"
    fi
    if [[ $rc -eq 0 ]]; then
        rm -rf "${TEST_BASE}"
    fi
}
trap cleanup EXIT
on_err() {
    local rc=$?
    local line="${BASH_LINENO[0]}"
    echo "=== E2E FAILED at line ${line} (exit ${rc}) ===" >&2
    exit "${rc}"
}
trap on_err ERR

# ---- step 0: build the per-run test root --------------------------------

mkdir -p "${STUB_DIR}" "${RAW_ROOT}" "${TEST_BASE}/cal"
echo "--- preflight: test root prepared ---"
echo "  test base: ${TEST_BASE}"
echo "  stub dir:  ${STUB_DIR}"
echo "  raw root:  ${RAW_ROOT}"
echo "  today:     ${TODAY}"

# ---- step 1: build the stub `gh` ---------------------------------------
#
# The github collector shells out to `gh` for three subcommands:
#   1. `gh search prs --author @me --state all --created <since>..<until>
#       --json number,title,state,url,createdAt,updatedAt,mergedAt,reviews
#       --limit 100`
#   2. `gh pr view <N> --json number,title,state,url,createdAt,updatedAt,mergedAt,reviews`
#   3. `gh pr view <N> --json commits`
#
# Per the per-source collector code, the shape of the JSON returned is
# what the synth module then transforms. We return the in-tree
# fixtures for the search/view/commits shapes. Because the supervisor
# calls `gh` for EVERY PR number from the search results (N pr views
# + N pr commits), the stub's view+commits handler accepts any PR
# number and returns the same fixture — the synth step then iterates
# over the search results, so the same fixture is the only PR that
# needs to be valid.
#
# `--hostname github.com` is suppressed by the collector (public
# github.com), so the stub doesn't need to handle it. If a v2 ever
# adds a per-PR `--hostname` argument, the stub will need to ignore
# it via the bash positional shift below.

cat > "${STUB_DIR}/gh" <<'STUB'
#!/bin/sh
# Stub `gh` for the e2e: returns canned JSON for the three subcommands
# the github collector uses. Anything else is a hard error so the
# test fails loudly if the collector starts invoking new shapes.

# Skip past `--hostname <host>` if present. The collector suppresses
# this on github.com, but be defensive in case a v2 re-enables it.
while [ $# -gt 0 ]; do
    case "$1" in
        --hostname) shift 2 ;;
        *) break ;;
    esac
done

FIXTURE_DIR="${TRAIL_E2E_FIXTURE_DIR:-/nonexistent}"

case "$1 $2" in
    "search prs")
        cat "${FIXTURE_DIR}/gh_search_author.json"
        exit 0
        ;;
    "pr view")
        # The third arg is the PR number; the collector always passes
        # the `--json <fields>` flag as a later arg. We don't care
        # which fields — return the same fixture regardless.
        if echo "$@" | grep -q "commits"; then
            cat "${FIXTURE_DIR}/gh_prs_commits.json"
        else
            cat "${FIXTURE_DIR}/gh_prs_view.json"
        fi
        exit 0
        ;;
    *)
        echo "stub gh: unknown invocation: $*" >&2
        exit 2
        ;;
esac
STUB
chmod +x "${STUB_DIR}/gh"
export TRAIL_E2E_FIXTURE_DIR="${REPO_ROOT}/crates/trail-collector/tests/fixtures/github"

# Pre-flight the stub: it should return a parseable JSON with the
# expected top-level keys.
ORIGINAL_PATH="${PATH}"
export PATH="${STUB_DIR}:${PATH}"
echo "--- preflight: stub gh ---"
gh_out="$(gh search prs --author @me --state all --created 2020-01-01T00:00:00Z..2030-01-01T00:00:00Z --json number,title,state,url,createdAt,updatedAt,mergedAt,reviews --limit 100)"
echo "  search output: ${gh_out:0:80}..."
echo "$gh_out" | "${PY}" -c "import sys, json; d=json.load(sys.stdin); assert 'items' in d, d; print(f'  stub gh: search returns {len(d[\"items\"])} PR(s) (OK)')"

gh_out2="$(gh pr view 142 --json number,title,state,url,createdAt,updatedAt,mergedAt,reviews)"
echo "$gh_out2" | "${PY}" -c "import sys, json; d=json.load(sys.stdin); assert 'reviews' in d, d; print(f'  stub gh: pr view returns {len(d[\"reviews\"])} review(s) (OK)')"

gh_out3="$(gh pr view 142 --json commits)"
echo "$gh_out3" | "${PY}" -c "import sys, json; d=json.load(sys.stdin); assert 'commits' in d, d; print(f'  stub gh: pr commits returns {len(d[\"commits\"])} commit(s) (OK)')"

# PATH is still prefixed with the stub; all subsequent gh invocations
# from the collector use the stub.

# ---- step 2: build per-source laptop configs ----------------------------
#
# Each per-source config is a CollectorLaptopConfig. The collector
# reads `source` and the fields the selected source actually uses;
# unused fields are ignored (the per-source run() pulls only what it
# needs from the cfg struct).

cat > "${TEST_BASE}/laptop-github.json" <<EOF
{
  "source": "github",
  "github": { "mode": "gh_cli", "host": "github.com", "enabled": true },
  "claude_sessions_paths": [],
  "calendar_ics": "/dev/null",
  "raw_root":  "${RAW_ROOT}",
  "schema_path": "${RESOURCES}/github.schema.json"
}
EOF

cat > "${TEST_BASE}/laptop-claude.json" <<EOF
{
  "source": "claude_sessions",
  "github": { "mode": "gh_cli", "host": "github.com", "enabled": false },
  "claude_sessions_paths": ["${REPO_ROOT}/crates/trail-collector/tests/fixtures/claude_sessions"],
  "calendar_ics": "/dev/null",
  "raw_root":  "${RAW_ROOT}",
  "schema_path": "${RESOURCES}/claude_sessions.schema.json"
}
EOF

# Copy the in-tree .ics fixture into the test root so a re-run on a
# different day still sees the right calendar file (the synth
# `today` filter compares against the collector's wall clock; the
# fixture's events are dated 2026-07-31, so this test asserts
# "this run on the day the fixture is dated OR a future day"). We
# pin the laptop's `today` to the fixture's date via a tiny
# adjustment: we leave the fixture as-is, and the test asserts that
# the calendar collector runs to completion (writes the file). For
# day-aware content checks we read the captured_at / date fields
# rather than asserting a specific event count — the structural
# re-validator in step 5 is the load-bearing check.
cp "${FIXTURE_CAL}" "${TEST_BASE}/cal/workday.ics"

cat > "${TEST_BASE}/laptop-calendar.json" <<EOF
{
  "source": "calendar",
  "github": { "mode": "gh_cli", "host": "github.com", "enabled": false },
  "claude_sessions_paths": [],
  "calendar_ics": "${TEST_BASE}/cal/workday.ics",
  "raw_root":  "${RAW_ROOT}",
  "schema_path": "${RESOURCES}/calendar.schema.json"
}
EOF

# A second claude-sessions run pointing at a fixture JSONL that uses
# today's date (Local) so the today-only filter keeps the sessions.
# The in-tree fixture is dated 2026-07-31, which is yesterday in CEST
# — the test that walks the in-tree fixture therefore exercises the
# "no sessions for today" branch of the today-filter, which is itself
# a load-bearing behavior (pre-Phase 6 onboarding + a stale config).
# This second run exercises the "sessions for today" branch with
# the in-tree synthesizer + the real on-disk roundtrip.
mkdir -p "${TEST_BASE}/claude-today"
TODAY_HUMAN="${TODAY}"
# Two timestamps so the lexicographic-compare in synth_claude picks
# the assistant (the later of the two) as the last_message. Identical
# timestamps would keep whichever row the BTreeMap saw first (the
# user prompt), defeating the e2e assertion that the assistant
# message is captured.
USER_TS="${TODAY_HUMAN}T10:00:00Z"
ASST_TS="${TODAY_HUMAN}T10:00:01Z"
cat > "${TEST_BASE}/claude-today/sessions.jsonl" <<EOF
{"sessionId":"e2e-sess-1","cwd":"/Users/pedro/work/e2e","type":"user","message":{"role":"user","content":"E2E smoke test message"},"timestamp":"${USER_TS}"}
{"sessionId":"e2e-sess-1","cwd":"/Users/pedro/work/e2e","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"E2E assistant reply — collector round-trip is up."}]},"timestamp":"${ASST_TS}"}
EOF
cat > "${TEST_BASE}/laptop-claude-today.json" <<EOF
{
  "source": "claude_sessions",
  "github": { "mode": "gh_cli", "host": "github.com", "enabled": false },
  "claude_sessions_paths": ["${TEST_BASE}/claude-today"],
  "calendar_ics": "/dev/null",
  "raw_root":  "${RAW_ROOT}",
  "schema_path": "${RESOURCES}/claude_sessions.schema.json"
}
EOF

echo
echo "--- laptop configs ready ---"
echo "  github:           ${TEST_BASE}/laptop-github.json"
echo "  claude (yest-fixture): ${TEST_BASE}/laptop-claude.json"
echo "  claude (today-fixture): ${TEST_BASE}/laptop-claude-today.json"
echo "  calendar:         ${TEST_BASE}/laptop-calendar.json"

# ---- step 3: invoke github collector (stub gh in PATH) -----------------

echo
echo "--- 1. github-collector against stub gh ---"
set +e
"${TRAIL_E2E_BINARY}" --config /dev/null collect --source github --laptop-config "${TEST_BASE}/laptop-github.json"
code_gh=$?
set -e
echo "  exit: ${code_gh}"
if [[ "${code_gh}" -ne 0 ]]; then
    echo "FATAL: github collector exited ${code_gh}" >&2
    exit 1
fi

GH_OUT="${DAY_DIR}/github.json"
[[ -f "${GH_OUT}" ]] || { echo "FATAL: github collector did not write ${GH_OUT}" >&2; exit 1; }
echo "  wrote: ${GH_OUT}"
"${PY}" -c "
import json
d = json.load(open('${GH_OUT}'))
assert d['source'] == 'github', d
prs = d['payload']['prs']
assert len(prs) >= 1, prs
states = sorted({p['state'] for p in prs})
print(f'  github envelope: source={d[\"source\"]} date={d[\"date\"]} prs={len(prs)} states={states} (OK)')
"

# ---- step 4: invoke claude-sessions collector ---------------------------

echo
echo "--- 2. claude-sessions-collector against fixture JSONL ---"
set +e
"${TRAIL_E2E_BINARY}" --config /dev/null collect --source claude-sessions --laptop-config "${TEST_BASE}/laptop-claude.json"
code_claude=$?
set -e
echo "  exit: ${code_claude}"
if [[ "${code_claude}" -ne 0 ]]; then
    echo "FATAL: claude-sessions collector exited ${code_claude}" >&2
    exit 1
fi

CLAUDE_OUT="${DAY_DIR}/claude_sessions.json"
[[ -f "${CLAUDE_OUT}" ]] || { echo "FATAL: claude-sessions collector did not write ${CLAUDE_OUT}" >&2; exit 1; }
echo "  wrote: ${CLAUDE_OUT}"
"${PY}" -c "
import json
d = json.load(open('${CLAUDE_OUT}'))
assert d['source'] == 'claude_sessions', d
sessions = d['payload']['sessions']
# The in-tree fixture is dated 2026-07-31, which is yesterday in CEST
# (the host's TZ), so the today-only filter legitimately drops every
# session. This is the 'no sessions for today' branch — pre-Phase 6
# onboarding + a stale config.
print(f'  claude_sessions (yesterday-fixture) envelope: source={d[\"source\"]} date={d[\"date\"]} sessions={len(sessions)} (today-filter applied as expected)')
"

# Step 2b: same source, but with a today-dated fixture so the
# today-only filter keeps the sessions. This exercises the full
# read-JSONL → synthesize → write path, not just the empty envelope.
echo
echo "--- 2b. claude-sessions-collector with today-dated fixture ---"
set +e
"${TRAIL_E2E_BINARY}" --config /dev/null collect --source claude-sessions --laptop-config "${TEST_BASE}/laptop-claude-today.json"
code_claude_today=$?
set -e
echo "  exit: ${code_claude_today}"
if [[ "${code_claude_today}" -ne 0 ]]; then
    echo "FATAL: claude-sessions (today-fixture) collector exited ${code_claude_today}" >&2
    exit 1
fi

CLAUDE_TODAY_OUT="${DAY_DIR}/claude_sessions.json"
[[ -f "${CLAUDE_TODAY_OUT}" ]] || { echo "FATAL: claude-sessions (today-fixture) collector did not write ${CLAUDE_TODAY_OUT}" >&2; exit 1; }
echo "  wrote: ${CLAUDE_TODAY_OUT} (overwrites yesterday-fixture result — fine for the e2e)"
"${PY}" -c "
import json
d = json.load(open('${CLAUDE_TODAY_OUT}'))
assert d['source'] == 'claude_sessions', d
sessions = d['payload']['sessions']
assert len(sessions) == 1, f'expected 1 session for today, got {len(sessions)}: {sessions}'
assert sessions[0]['session_id'] == 'e2e-sess-1', sessions[0]
assert sessions[0]['message_count'] == 2, sessions[0]
assert sessions[0]['last_message']['role'] == 'assistant', sessions[0]
# Privacy: the synth keeps only the LATEST message's headline (≤280 chars).
# The body is short, so the headline equals the body.
headline = sessions[0]['last_message']['content_headline']
assert headline == 'E2E assistant reply — collector round-trip is up.', f'expected the assistant text; got {headline!r}'
# Privacy: the user message text must NOT appear in the payload — the
# synth keeps only the last (assistant) message, never the user prompt.
payload_str = json.dumps(d)
assert 'E2E smoke test message' not in payload_str, f'user prompt leaked into payload: {payload_str}'
print(f'  claude_sessions (today-fixture) envelope: 1 session, last_message.role=assistant, user prompt NOT in payload (OK)')
"

# ---- step 5: invoke calendar collector ---------------------------------

echo
echo "--- 3. calendar-collector against fixture .ics ---"
set +e
"${TRAIL_E2E_BINARY}" --config /dev/null collect --source calendar --laptop-config "${TEST_BASE}/laptop-calendar.json"
code_cal=$?
set -e
echo "  exit: ${code_cal}"
if [[ "${code_cal}" -ne 0 ]]; then
    echo "FATAL: calendar collector exited ${code_cal}" >&2
    exit 1
fi

CAL_OUT="${DAY_DIR}/calendar.json"
[[ -f "${CAL_OUT}" ]] || { echo "FATAL: calendar collector did not write ${CAL_OUT}" >&2; exit 1; }
echo "  wrote: ${CAL_OUT}"
"${PY}" -c "
import json
d = json.load(open('${CAL_OUT}'))
assert d['source'] == 'calendar', d
events = d['payload']['events']
print(f'  calendar envelope: source={d[\"source\"]} date={d[\"date\"]} events={len(events)} (OK)')
# Privacy rule: DESCRIPTION body must NEVER appear in any field.
payload_str = json.dumps(d)
forbidden = ['Discuss the wizard variants', 'Discuss career goals', 'Should not appear']
for f in forbidden:
    assert f not in payload_str, f'DESCRIPTION body leaked: {f!r} in {payload_str}'
print('  privacy rule: DESCRIPTION body NOT present (OK)')
"

# ---- step 6: re-validate the written files against the bundled schemas --

echo
echo "--- 4. re-validate written JSON against bundled schemas ---"

re_validate() {
    local source="$1"
    local data_path="$2"
    local schema_path="$3"
    if [[ "${HAVE_JSONSCHEMA}" -eq 1 ]]; then
        "${PY}" - "$data_path" "$schema_path" <<'PYEOF'
import json
import sys

import jsonschema

data = json.load(open(sys.argv[1]))
schema = json.load(open(sys.argv[2]))
try:
    jsonschema.validate(data, schema)
    print(f"  {sys.argv[1].split('/')[-1]}: jsonschema validation passed")
except jsonschema.ValidationError as e:
    print(f"  FATAL: {sys.argv[1]}: jsonschema validation FAILED: {e.message}", file=sys.stderr)
    sys.exit(1)
PYEOF
    else
        "${PY}" - "$data_path" "$schema_path" "$source" <<'PYEOF'
import json
import sys

data = json.load(open(sys.argv[1]))
required_root = ("source", "captured_at", "date", "payload")
for k in required_root:
    if k not in data:
        print(f"  FATAL: {sys.argv[1]}: missing required root key {k!r}", file=sys.stderr)
        sys.exit(1)
if data["source"] != sys.argv[3]:
    print(f"  FATAL: {sys.argv[1]}: source={data['source']!r} (expected {sys.argv[3]!r})", file=sys.stderr)
    sys.exit(1)
print(f"  {sys.argv[1].split('/')[-1]}: structural sanity passed (install python jsonschema for the real check)")
PYEOF
    fi
}

re_validate github    "${GH_OUT}"     "${RESOURCES}/github.schema.json"
re_validate claude    "${CLAUDE_OUT}" "${RESOURCES}/claude_sessions.schema.json"
re_validate calendar  "${CAL_OUT}"    "${RESOURCES}/calendar.schema.json"

# ---- success ------------------------------------------------------------

echo
echo "=== PHASE 2 E2E PASSED ==="
echo "  test base:        ${TEST_BASE}"
echo "  raw root:         ${RAW_ROOT}"
echo "  day dir:          ${DAY_DIR}"
echo "  github stub:      ${STUB_DIR}/gh"
echo "  artifacts:"
echo "    ${GH_OUT}"
echo "    ${CLAUDE_OUT}"
echo "    ${CAL_OUT}"
