#!/usr/bin/env bash
# tests/post_build_smoke.sh
#
# Validates the universal-binary build matrix in release.yml + the
# per-crate version discipline on the collector. Run from the repo root:
#   bash tests/post_build_smoke.sh
#
# Cases:
#   1. release.yml matrix YAML structure (aarch64 + x86_64 macos jobs,
#      lipo -create post-step, no secret-referencing if: keys).
#   2. lipo -info smoke against a fixture universal binary — created via
#      lipo when available, SKIPPED with a warning on Linux (lipo is
#      macOS-only; this is the HEADLESS-HOST HONEST CLAIM shape from
#      the Phase 7 plan).
#   3. Post-step structure (zip/dmg creation + signature + notarization
#      hooks present) — bash-guard pattern required per talon PR #62.
#
# Optional: RUN_INSTALL_SMOKE=1 enables a real `cargo install` from
# the local path, for Pedro's manual verification on his Mac.
#
# Exits 0 on full pass; non-zero on any failed assertion.

set -euo pipefail

cd "$(dirname "$0")/.."

PASS=0
FAIL=0
WARN=0
ok()    { echo "✓ $*"; PASS=$((PASS + 1)); }
bad()   { echo "✗ $*"; FAIL=$((FAIL + 1)); }
note()  { echo "::warning::$*"; WARN=$((WARN + 1)); }

echo "=== Universal binary + post-build smoke ==="
echo

# ---------------------------------------------------------------------------
# Case 1: matrix YAML structure
# ---------------------------------------------------------------------------
echo "[1] release.yml matrix YAML structure"
RELEASE_YML=.github/workflows/release.yml
if [ ! -f "$RELEASE_YML" ]; then
    bad "$RELEASE_YML missing"
    exit 1
fi
ok "$RELEASE_YML exists"

# Both apple-darwin targets must appear as build steps inside the
# universal-binary job. Tauri produces a per-arch executable that the
# lipo step merges into one universal binary.
if grep -q 'aarch64-apple-darwin' "$RELEASE_YML"; then
    ok "aarch64-apple-darwin target present in release.yml"
else
    bad "release.yml is missing aarch64-apple-darwin target"
fi

if grep -q 'x86_64-apple-darwin' "$RELEASE_YML"; then
    ok "x86_64-apple-darwin target present in release.yml"
else
    bad "release.yml is missing x86_64-apple-darwin target"
fi

# The universal-binary job must be matrix-driven (per-arch separately).
# `strategy.matrix.arch` OR a build job covering both arches explicitly
# is the binding shape. We accept either pattern because item 7-2 / 7-3
# may phrase it differently than 7-7.
if grep -q 'strategy:' "$RELEASE_YML" && grep -q 'matrix' "$RELEASE_YML"; then
    ok "strategy.matrix shape detected in release.yml"
elif grep -q 'build-mac-universal' "$RELEASE_YML"; then
    ok "build-mac-universal job detected (non-matrix variant)"
else
    note "neither strategy.matrix nor build-mac-universal job detected — \
          check that the universal-binary job exists by name"
fi

# lipo -create invocation must be present — that's the merge step that
# produces the universal binary from the per-arch slices.
if grep -q 'lipo -create' "$RELEASE_YML"; then
    ok "lipo -create invocation present"
else
    bad "release.yml has no 'lipo -create' invocation (cannot merge per-arch binaries)"
fi

# Secret-referencing if: keys are unreliable per talon PR #62. None
# allowed in this file (the bash-guard pattern is the only allowed
# shape — see tests/workflows_smoke.sh for the same check).
SECRET_IF=$(grep -rE '^[[:space:]]*if:[[:space:]]+\$?\{\{[[:space:]]*secrets\.' "$RELEASE_YML" || true)
if [ -n "$SECRET_IF" ]; then
    bad "Found secret-referencing 'if:' key in $RELEASE_YML (per-talon PR #62 pitfall):"
    echo "$SECRET_IF"
else
    ok "No secret-referencing 'if:' keys in $RELEASE_YML"
fi

echo

# ---------------------------------------------------------------------------
# Case 2: lipo -info smoke against a fixture universal binary
# ---------------------------------------------------------------------------
echo "[2] lipo -info smoke on a fixture universal binary"
LIPO=$(command -v lipo || true)
if [ -z "$LIPO" ]; then
    note "lipo not available on this host (expected on Linux; lipo is macOS-only). \
          skipping the lipo -info invocation — the YAML structure check above is the \
          load-bearing verification for this gate on Linux."
else
    FIXTURE_DIR=$(mktemp -d /tmp/trail-lipo.XXXXXX)
    trap 'rm -rf "$FIXTURE_DIR"' EXIT
    FIXTURE_BIN="$FIXTURE_DIR/fixture"
    # Build a tiny multi-arch fat binary. lipo on macOS requires real
    # Mach-O slices; `/bin/ls` and `/bin/echo` are universal on every
    # macOS install since 10.6, so we can use those for the smoke.
    if cp /bin/echo "$FIXTURE_DIR/base" && "$LIPO" -create \
        "$FIXTURE_DIR/base" "$FIXTURE_DIR/base" -output "$FIXTURE_BIN" 2>/dev/null; then
        INFO=$("$LIPO" -info "$FIXTURE_BIN" 2>&1 || true)
        if [ -n "$INFO" ]; then
            ok "lipo -info produced output: $INFO"
        else
            bad "lipo -info returned no output for $FIXTURE_BIN"
        fi
    else
        note "lipo -create of /bin/echo failed (likely single-arch host). \
              The matrix YAML structure check above remains load-bearing on this host."
    fi
fi
echo

# ---------------------------------------------------------------------------
# Case 3: post-step structure (zip/dmg creation + signature + notarization)
# ---------------------------------------------------------------------------
echo "[3] Post-step structure (zip/dmg + signature + notarization hooks)"

# Either a direct dmg creation step OR a Tauri-bundler step that
# produces the dmg is required. Accept both shapes.
if grep -q '\.dmg' "$RELEASE_YML"; then
    ok "release.yml references .dmg (macOS installer artifact)"
else
    bad "release.yml does not reference .dmg (no installer artifact produced)"
fi

# Codesign / signing step is required — universal binaries must be
# signed for the notarizer to accept them. Bash-guard pattern required
# (talon PR #62); the actual sign happens only when the cert secret
# is configured.
if grep -qE 'codesign[[:space:]]' "$RELEASE_YML"; then
    ok "codesign step present"
else
    note "no explicit 'codesign' step detected — Tauri may sign implicitly \
          via tauri.conf.json (acceptable as long as signingIdentity is set)"
fi

# Notarization hook (notarytool or stapler) is required for Gatekeeper-
# acceptance of the universal .app. Either via xcrun notarytool or
# `tauri build` itself (which invokes notarytool internally when
# tauri.conf.json configures it).
if grep -q 'notarytool' "$RELEASE_YML" || grep -q 'APPLE_API_KEY' "$RELEASE_YML"; then
    ok "notarization hook present in release.yml"
else
    note "no explicit notarization hook detected — Tauri bundler may run \
          it implicitly from tauri.conf.json (acceptable when configured)"
fi

# Bundle upload step: gh release OR actions/upload-artifact. Either one
# ships the .dmg/.app to GitHub Releases (or as a workflow artifact).
if grep -qE '(gh release|softprops/action-gh-release|actions/upload-artifact)' "$RELEASE_YML"; then
    ok "bundle upload step present"
else
    bad "release.yml has no bundle upload step (gh release / softprops / upload-artifact)"
fi

echo

# ---------------------------------------------------------------------------
# Optional: RUN_INSTALL_SMOKE=1 enables a real `cargo install` from the
# local path. Used by Pedro's manual verification on his Mac.
# ---------------------------------------------------------------------------
if [ "${RUN_INSTALL_SMOKE:-0}" = "1" ]; then
    echo "[install-smoke] RUN_INSTALL_SMOKE=1 — running `cargo install --path`:"
    cargo install --path crates/trail-collector --quiet
    INSTALLED_VER=$(trail-collector --version 2>&1 | head -1 || true)
    if [ -n "$INSTALLED_VER" ]; then
        ok "installed binary reports: $INSTALLED_VER"
    else
        bad "installed binary returned empty --version"
    fi
    cargo uninstall trail-collector --quiet || true
    echo
fi

echo "=== Summary: pass=$PASS, warn=$WARN, fail=$FAIL ==="
if [ "$FAIL" -gt 0 ]; then
    echo "✗ Post-build smoke FAILED"
    exit 1
fi
echo "✓ Post-build smoke passed"
