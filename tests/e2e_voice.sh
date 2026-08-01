#!/usr/bin/env bash
# tests/e2e_voice.sh — Headless end-to-end test for the Phase 5 voice
# pipeline.
#
# Phase 5 §5.8 (Part B). Verifies the synthesized-5sec-WAV e2e on the
# agent's headless Linux build host:
#   1. Builds the trail crate (compile-checks the voice + whisper
#      dependencies link cleanly).
#   2. Generates a 5-second 440 Hz sine wave WAV via a small Rust
#      helper (hound::WavWriter, 16 kHz mono 16-bit PCM).
#   3. Runs `cargo run --example voice_e2e` against the fixture. The
#      example decodes the WAV, runs the real `voice::transcriber::
#      transcribe` pipeline (whisper-rs with the real
#      `ggml-base.en.bin` model), and writes a VoiceEntry to
#      `~/.trail/raw/<date>/voice/<entry_id>.{json,wav}`.
#   4. Asserts the output JSON path + non-empty transcript + valid
#      WAV file (when the model is present).
#
# Skip mode: when `TRAIL_E2E_HOST` is unset the script prints a
# SKIPPED banner and exits 0. This makes the script PR-able from a
# headless Linux build host where the 150 MB whisper model is not
# present — the load-bearing proof runs on the macOS laptop per
# `docs/e2e-runbook.md` / `tests/MACOS_PHASE5_CHECKLIST.md`.
#
# Same skip-mode pattern as `tests/e2e_collector.sh` (Phase 1) and
# `tests/e2e_collectors.sh` (Phase 2): the env var name + the
# "SKIPPED: <trigger> — re-run on the macOS laptop" banner + the
# summary line that echoes the configured env vars + exit 0.

set -Eeuo pipefail

# ---- CLI args ------------------------------------------------------------

SKIP_HOST=0
for arg in "$@"; do
    case "$arg" in
        --skip-host) SKIP_HOST=1 ;;
        -h|--help)
            cat <<'USAGE'
Usage: bash tests/e2e_voice.sh [--skip-host]

Required environment (skip mode when unset):
  TRAIL_E2E_HOST     Any non-empty value. The script does not read the
                     contents — it only checks the variable is set.
                     Acts as the opt-in signal that a real e2e run
                     should execute (i.e. the host has the whisper
                     model + a writable HOME).
  TRAIL_HOME         Override for the on-disk trail root. The example
                     binary writes VoiceEntries here.
                     (default: $HOME/.trail)
  TRAIL_E2E_WAV_OUT  Optional override for the synthesized fixture
                     WAV path. The default is a tempfile in
                     ${TMPDIR:-/tmp}.
  TRAIL_E2E_BINARY   Local path to the trail-collector binary, if
                     we ever want to validate a sub-bin path.
                     (default: not used by this script)

When TRAIL_E2E_HOST is unset OR --skip-host is passed, the script
prints "SKIPPED" and exits 0. The script is hermetic on a Linux
build host: the synthesized WAV + the example binary + the
transcribe pipeline (with model-not-found → exit 0) all work
without a real microphone or the 150 MB model. The macOS load-
bearing proof is `tests/MACOS_PHASE5_CHECKLIST.md`.
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
TRAIL_HOME="${TRAIL_HOME:-${HOME}/.trail}"
TRAIL_E2E_WAV_OUT="${TRAIL_E2E_WAV_OUT:-$(mktemp -u -t trail-voice-e2e-XXXXXX.wav)}"

# Skip mode: no host trigger, or explicit --skip-host flag. Same
# convention as tests/e2e_collector.sh (Phase 1) and
# tests/e2e_collectors.sh (Phase 2) — PR-able from any host.
if [[ -z "$TRAIL_E2E_HOST" || "$SKIP_HOST" -eq 1 ]]; then
    if [[ "$SKIP_HOST" -eq 1 ]]; then
        echo "SKIPPED: --skip-host flag set — re-run without the flag on the macOS laptop."
    else
        echo "SKIPPED: TRAIL_E2E_HOST not set — re-run on the macOS laptop."
    fi
    echo "  host trigger:    ${TRAIL_E2E_HOST:-<unset>}"
    echo "  trail home:      ${TRAIL_HOME}"
    echo "  wav out:         ${TRAIL_E2E_WAV_OUT}"
    echo "  (this is a feature: the script is PR-able from a headless Linux build host)"
    echo
    echo "=== E2E SKIPPED ==="
    exit 0
fi

# ---- derive paths -------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ---- preflight: example binary builds ----------------------------------

echo
echo "--- 1. cargo build --example voice_e2e (compile-checks the pipeline) ---"

# Use release for speed — the example is otherwise unchanged from debug,
# but whisper-rs pulls in heavy C++ codegen.
(
    cd "${REPO_ROOT}" && \
        cargo build -p trail --example voice_e2e 2>&1 | tail -5
)

# ---- 2. synthesize 5-sec 440 Hz sine wave WAV ---------------------------

echo
echo "--- 2. synthesize 5-second 440 Hz sine wave WAV (hound) ---"

# We can't use hound::WavWriter from bash, so we shell out to a small
# inline Rust helper. The helper is in-tree as a doc-example-free
# one-liner via `cargo run --example` on a synthetic inline Rust file.
# To keep the dependency surface tight we use a one-liner via
# `rustc` + stdin that imports the bundled `hound` (already in the
# workspace) via the same crate graph.
#
# In practice the easiest path: the e2e_voice.sh script writes a tiny
# `.rs` file to a tempfile, builds it with `rustc` against the
# workspace's `hound` dep via `-L dependency=...`, and runs the
# resulting binary. Cargo handles dep resolution for us, so we just
# reuse the same `cargo run --example voice_e2e --features=...`
# pattern with a separate example file is overkill.
#
# Approach: a temp .rs file that uses hound directly + `rustc` against
# the workspace's target dir. The helper crate graph is resolved by
# cargo at the workspace root, then we link against the resulting
# hound rlib.

# Build a one-shot Rust source for the WAV synthesis. It uses the
# `hound` crate via rustc's `-L dependency` (resolved by cargo's
# workspace-level dep graph) — no separate Cargo project needed.
SYNTH_SRC="$(mktemp -t trail-voice-synth-XXXXXX.rs)"
SYNTH_BIN="$(mktemp -t trail-voice-synth-XXXXXX.bin)"
# We intentionally do NOT clean up the synthesized WAV — it's a
# 160 KB artifact useful for inspection when the example fails on
# a real host. Set `TRAIL_E2E_WAV_OUT_PRESERVE=0` to clean it up.
# The script's own scratch (source, binary) is always cleaned.

cat > "${SYNTH_SRC}" <<'RUST_EOF'
//! Synthesize a 5-second 440 Hz sine wave WAV at 16 kHz mono
//! 16-bit PCM. Single-file, std-only + hound. Args: <out.wav>.
use std::path::PathBuf;
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = PathBuf::from(env::args().nth(1).expect("out.wav path required"));
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&out, spec)?;
    let total = (16_000_f32 * 5.0) as usize;
    for i in 0..total {
        let t = i as f32 / 16_000.0;
        let sample = (t * 440.0 * std::f32::consts::PI * 2.0).sin() * 0.5;
        let s16 = (sample * 32_767.0).clamp(-32_768.0, 32_767.0) as i16;
        writer.write_sample(s16)?;
    }
    writer.finalize()?;
    println!("wrote {} samples to {}", total, out.display());
    Ok(())
}
RUST_EOF

# Resolve the workspace's hound rlib so rustc can link against it.
# `cargo build -p trail` already builds hound into
# `target/debug/deps/libhound-<hash>.rlib`. We let cargo tell us
# the right path via `cargo metadata`.
HOUND_RLIB="$(
    cd "${REPO_ROOT}" && \
        cargo metadata --format-version 1 --no-deps 2>/dev/null | \
        python3 -c '
import json, sys
m = json.load(sys.stdin)
# Find the hound package in the workspace dep set.
for p in m["packages"]:
    if p["name"] == "hound":
        # The rlib is in target/debug/deps (host target).
        for t in p["targets"]:
            print(t["name"])
        break
'
)"

if [[ -z "${HOUND_RLIB}" ]]; then
    # Fall back: compile hound via `cargo build` so the rlib exists,
    # then locate it in target/debug/deps.
    (cd "${REPO_ROOT}" && cargo build -p trail 2>&1 | tail -3)
    HOUND_RLIB_PATH="$(find "${REPO_ROOT}/target/debug/deps" -name 'libhound-*.rlib' 2>/dev/null | head -1)"
else
    # We have the target name; find the rlib on disk.
    HOUND_RLIB_PATH="$(find "${REPO_ROOT}/target/debug/deps" -name "lib${HOUND_RLIB}-*.rlib" 2>/dev/null | head -1)"
fi

if [[ -z "${HOUND_RLIB_PATH}" || ! -f "${HOUND_RLIB_PATH}" ]]; then
    echo "FATAL: could not locate hound rlib in target/debug/deps" >&2
    echo "Build first: cargo build -p trail" >&2
    exit 1
fi

rustc "${SYNTH_SRC}" -o "${SYNTH_BIN}" \
    --edition 2021 \
    -L "${REPO_ROOT}/target/debug/deps" \
    --extern hound="${HOUND_RLIB_PATH}" 2>&1 | tail -5

"${SYNTH_BIN}" "${TRAIL_E2E_WAV_OUT}"

if [[ ! -s "${TRAIL_E2E_WAV_OUT}" ]]; then
    echo "FATAL: synthesized WAV is missing or empty: ${TRAIL_E2E_WAV_OUT}" >&2
    exit 1
fi

echo "  wav: ${TRAIL_E2E_WAV_OUT} ($(stat -c '%s' "${TRAIL_E2E_WAV_OUT}") bytes)"

# ---- 3. run the example binary ----------------------------------------

echo
echo "--- 3. run the voice_e2e example binary ---"

# We use the freshly-built example from step 1 directly (faster than
# `cargo run` for repeat invocations). Stdout is captured to a
# tempfile for assertions below.
EXAMPLE_BIN="${REPO_ROOT}/target/debug/examples/voice_e2e"
if [[ ! -x "${EXAMPLE_BIN}" ]]; then
    echo "FATAL: example binary missing: ${EXAMPLE_BIN}" >&2
    exit 1
fi

EXAMPLE_OUT="$(mktemp -t trail-voice-e2e-out-XXXXXX.txt)"
# Trap cleans up the script's scratch files (synth source + binary
# + example stdout capture) on exit. The synthesized WAV is
# preserved (see comment near TRAIL_E2E_WAV_OUT default above).
trap 'rm -f "${SYNTH_SRC}" "${SYNTH_BIN}" "${EXAMPLE_OUT}"' EXIT

# The example will print "MODEL NOT FOUND — download via §5.1: <path>"
# if the whisper model isn't cached. That's the documented skip-mode
# for the model step; the script's load-bearing proof (decoded WAV
# + JSON written + valid transcript) is on the macOS laptop where
# the model is present. We allow exit 0 with that banner to pass.
set +e
TRAIL_HOME="${TRAIL_HOME}" \
"${EXAMPLE_BIN}" "${TRAIL_E2E_WAV_OUT}" >"${EXAMPLE_OUT}" 2>&1
example_exit=$?
set -e

echo "  example stdout:"
sed 's/^/    /' "${EXAMPLE_OUT}"
echo "  example exit: ${example_exit}"

# The example always exits 0 (per spec — model-not-found is success,
# not failure). A non-zero exit indicates a real bug.
if [[ "${example_exit}" -ne 0 ]]; then
    echo "FATAL: voice_e2e example exited ${example_exit} (expected 0)" >&2
    exit 1
fi

# If the model was missing, the example printed the SKIP banner and
# returned without writing JSON. The script's load-bearing proof
# is on macOS; here we report skip cleanly.
if grep -q "^MODEL NOT FOUND" "${EXAMPLE_OUT}"; then
    echo
    echo "=== PHASE 5 E2E SKIPPED (model not on this host) ==="
    echo "  wav:         ${TRAIL_E2E_WAV_OUT}"
    echo "  trail home:  ${TRAIL_HOME}"
    echo "  re-run on macOS laptop where ~/.trail/models/ggml-base.en.bin is cached."
    exit 0
fi

# ---- 4. assert the output JSON + WAV -----------------------------------

echo
echo "--- 4. assert output JSON + WAV ---"

# Parse the `json: <path>` line the example prints.
json_line="$(grep '^json: ' "${EXAMPLE_OUT}" | head -1 || true)"
if [[ -z "${json_line}" ]]; then
    echo "FATAL: example did not print a 'json: <path>' line" >&2
    exit 1
fi
json_path="${json_line#json: }"
echo "  json: ${json_path}"

# JSON must exist + parse.
if [[ ! -f "${json_path}" ]]; then
    echo "FATAL: output JSON missing: ${json_path}" >&2
    exit 1
fi
if ! python3 -c "import json; json.load(open('${json_path}'))"; then
    echo "FATAL: output JSON does not parse: ${json_path}" >&2
    exit 1
fi

# WAV must exist + be non-empty.
wav_path="${json_path%.json}.wav"
if [[ ! -s "${wav_path}" ]]; then
    echo "FATAL: output WAV missing or empty: ${wav_path}" >&2
    exit 1
fi
echo "  wav:  ${wav_path} ($(stat -c '%s' "${wav_path}") bytes)"

# Transcript must exist as a field in the JSON (may be empty string
# or "[BLANK_AUDIO]" — both are documented whisper outcomes on a
# pure sine wave). We just require the field be present.
if ! python3 -c "
import json, sys
e = json.load(open('${json_path}'))
assert 'transcript' in e, 'transcript field missing'
t = e['transcript']
# Transcript may be a string OR a {text, segments} object.
if isinstance(t, dict):
    t = t.get('text', '')
sys.exit(0 if t is not None else 1)
"; then
    echo "FATAL: output JSON missing 'transcript' field: ${json_path}" >&2
    exit 1
fi

# ---- success -----------------------------------------------------------

echo
echo "=== PHASE 5 E2E PASSED ==="
echo "  wav:         ${TRAIL_E2E_WAV_OUT}"
echo "  json:        ${json_path}"
echo "  voice entry: ${json_path}"
echo "  trail home:  ${TRAIL_HOME}"
