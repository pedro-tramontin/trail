#!/usr/bin/env bash
# tests/workflows_smoke.sh
#
# Validates the GH Actions workflow files in this repo. Run from the
# repo root: `bash tests/workflows_smoke.sh`.
#
# Cases (Phase 7 + Phase 8 §8.1 cleanup):
#   1. .github/workflows/{release,draft-build}.yml are valid YAML.
#   2. yamllint exits 0 for both workflow files (if yamllint installed).
#   3. act -n exits 0 for both workflow files (if act installed).
#   4. Top-level `if:` keys do not reference secrets (per-talon PR #62).
#   5. Per-crate version invariant (no `version.workspace = true`).
#   6. Collector `cargo install` discoverability lint (item 7-3).
#   7. tauri.conf.json signing config is env-driven.
#   8. release.yml has upload-assets job (Phase 7 §7.7).
#   9. softprops/action-gh-release is SHA-pinned (supply-chain-audit).
#
# Phase 8 §8.2 EXTENDS this script with: release-drafter config
# validity, 3 new workflow files (release-drafter, version-bump,
# promote) YAML validity, and GITHUB_TOKEN-not-RELEASE_PLEASE_TOKEN
# check for the new workflows.
#
# The script exits 0 when structural validity holds; the optional
# yamllint + `act -n` checks skip with a `::warning::` line if the
# tool is not installed on the host (the headless Linux build host
# does not have either).

set -euo pipefail

cd "$(dirname "$0")/.."

PASS=0
FAIL=0
WARN=0

ok() { echo "✓ $*"; PASS=$((PASS + 1)); }
bad() { echo "✗ $*"; FAIL=$((FAIL + 1)); }
note_warn() { echo "::warning::$*"; WARN=$((WARN + 1)); }

echo "=== Workflow smoke ==="
echo

# --- Case 1: YAML structural validity ---
echo "[1] GitHub Actions YAML structural validity"
if command -v python3 >/dev/null 2>&1 && python3 -c "import yaml" 2>/dev/null; then
    for f in .github/workflows/release.yml .github/workflows/draft-build.yml; do
        if python3 -c "import yaml,sys; yaml.safe_load(open('$f'))" 2>/dev/null; then
            ok "$f parses as YAML"
        else
            bad "$f does not parse as YAML"
        fi
    done
else
    note_warn "python3+yaml not available; skipping YAML structural check"
fi

# --- Case 2: yamllint (optional) ---
echo
echo "[2] yamllint (optional)"
if command -v yamllint >/dev/null 2>&1; then
    set +e
    yamllint -d '{extends: default, rules: {line-length: disable}}' \
        .github/workflows/release.yml \
        .github/workflows/draft-build.yml
    rc=$?
    set -e
    if [ "$rc" -eq 0 ]; then
        ok "yamllint passes on both workflows"
    else
        bad "yamllint reported issues (exit $rc)"
    fi
else
    note_warn "yamllint not installed; skipping (install with: pip install yamllint)"
fi

# --- Case 3: act -n dry-run (optional) ---
echo
echo "[3] act -n dry-run (optional)"
if command -v act >/dev/null 2>&1; then
    set +e
    act -n -W .github/workflows/release.yml >/dev/null 2>&1
    rc_rp=$?
    act -n -W .github/workflows/draft-build.yml >/dev/null 2>&1
    rc_db=$?
    set -e
    if [ "$rc_rp" -eq 0 ] && [ "$rc_db" -eq 0 ]; then
        ok "act -n dry-run passes on both workflows"
    else
        bad "act -n failed (release.yml=$rc_rp, draft-build.yml=$rc_db)"
    fi
else
    note_warn "act not installed; skipping dry-run (install from https://nektosact.com/installation/)"
fi

# --- Pitfall check: top-level `if:` keys that reference secrets ---
echo
echo "=== Pitfall check: secret-referencing 'if:' keys (per-talon PR #62) ==="
SECRET_IF=$(grep -rE '^[[:space:]]*if:[[:space:]]+\$?\{\{[[:space:]]*secrets\.' .github/workflows/ || true)
if [ -n "$SECRET_IF" ]; then
    bad "Found secret-referencing 'if:' key (breaks tag-triggered workflow):"
    echo "$SECRET_IF"
else
    ok "No secret-referencing 'if:' keys"
fi

# --- Per-crate version invariant (Phase 7 §7.2 — src-tauri only) ---
# release-please can't bump workspace-inherited versions (it walks
# `extraFiles` in release-please-config.json and only handles inline
# `version = "X.Y.Z"`). Per talon issue #2111 every workspace member
# must declare an inline version. Item 7-2 enforces the src-tauri
# crate; the trail-collector crate's equivalent is item 7-3's scope
# (the universal-binary build).
echo
echo "=== Per-crate version invariant (no version.workspace = true) ==="
WORKSPACE_VERSION_CRATES=0
for crate_toml in src-tauri/Cargo.toml crates/trail-collector/Cargo.toml; do
    if [ -f "$crate_toml" ]; then
        hits=$(grep -c '^version\.workspace *= *true' "$crate_toml" || true)
        if [ "$hits" -gt 0 ]; then
            bad "$crate_toml uses version.workspace = true (per-crate inline required)"
            WORKSPACE_VERSION_CRATES=$((WORKSPACE_VERSION_CRATES + 1))
        else
            ok "$crate_toml uses inline version (not workspace-inherited)"
        fi
    fi
done
if [ "$WORKSPACE_VERSION_CRATES" -ne 0 ]; then
    : # bad() already incremented the FAIL counter
fi

# --- Collector `cargo install` discoverability lint (item 7-3) ---
# `cargo install trail-collector --git <repo>` shows a discoverable
# tile only when `[package]` declares homepage / repository / description
# (and a license). Missing fields give a blank install tile and a
# crates.io page (if ever published) without install instructions.
echo
echo "=== Collector cargo-install discoverability (item 7-3) ==="
COLLECTOR_TOML=crates/trail-collector/Cargo.toml
if [ ! -f "$COLLECTOR_TOML" ]; then
    note_warn "$COLLECTOR_TOML missing — skipping discoverability lint"
else
    if grep -q '^description = ' "$COLLECTOR_TOML"; then
        ok "collector has description"
    else
        bad "collector missing description field (cargo install tile is blank)"
    fi
    if grep -q '^homepage = ' "$COLLECTOR_TOML"; then
        ok "collector has homepage"
    else
        bad "collector missing homepage field"
    fi
    if grep -q '^repository = ' "$COLLECTOR_TOML"; then
        ok "collector has repository"
    else
        bad "collector missing repository field"
    fi
    if grep -qE '^(license|license\.workspace) *=' "$COLLECTOR_TOML"; then
        ok "collector has license declaration"
    else
        bad "collector missing license declaration"
    fi
fi

# --- tauri.conf.json signing-block lint (Phase 7 §7.2) ---
# The macOS signing identity + team ID are read from env vars at
# bundling time (Tauri's ${VAR} expansion). Hardcoding the values
# would leak the signing identity into the public repo.
echo
echo "=== tauri.conf.json signing config (env-driven) ==="
if ! command -v jq >/dev/null 2>&1; then
    note_warn "jq not installed; skipping tauri.conf.json signing-block lint"
else
    SIGN_IDENTITY=$(jq -r '.bundle.macOS.signingIdentity // empty' src-tauri/tauri.conf.json)
    if [ "$SIGN_IDENTITY" != '${APPLE_SIGNING_IDENTITY}' ]; then
        bad "tauri.conf.json bundle.macOS.signingIdentity is not env-driven"
        echo "  found: '$SIGN_IDENTITY'"
        echo "  expected: '\${APPLE_SIGNING_IDENTITY}'"
    else
        ok "signingIdentity is env-driven: $SIGN_IDENTITY"
    fi
    TEAM_ID=$(jq -r '.bundle.macOS.providerShortName // empty' src-tauri/tauri.conf.json)
    if [ "$TEAM_ID" != '${APPLE_TEAM_ID}' ]; then
        bad "tauri.conf.json bundle.macOS.providerShortName is not env-driven"
        echo "  found: '$TEAM_ID'"
        echo "  expected: '\${APPLE_TEAM_ID}'"
    else
        ok "providerShortName is env-driven: $TEAM_ID"
    fi
    ENTITLEMENTS=$(jq -r '.bundle.macOS.entitlements // empty' src-tauri/tauri.conf.json)
    if [ "$ENTITLEMENTS" != '${TRAIL_ENTITLEMENTS_PATH}' ]; then
        bad "tauri.conf.json bundle.macOS.entitlements is not env-driven"
        echo "  found: '$ENTITLEMENTS'"
        echo "  expected: '\${TRAIL_ENTITLEMENTS_PATH}'"
    else
        ok "entitlements is env-driven: $ENTITLEMENTS"
    fi
fi

# --- upload-assets YAML structure (Phase 7 §7.7) ---
# The release.yml file must contain a top-level `upload-assets` job
# that depends on `release-please` + the build jobs. Pinning + the
# artifact-upload step shape are the load-bearing checks.
echo
echo "=== upload-assets job structure (Phase 7 §7.7) ==="

if grep -qE '^[[:space:]]+upload-assets:[ ]*$' .github/workflows/release.yml; then
    ok "release.yml has upload-assets job"
else
    bad "release.yml missing upload-assets job"
fi

# Window: from `upload-assets:` line, take the next 8 lines.
# Note: GNU grep 3.11 has a quirk where `+` in BRE (`grep` without
# `-E`) is treated as a literal at the start of a pattern. Use
# `grep -E` (ERE) consistently or use `\{1,\}` (BRE repetition).
UPLOAD_WINDOW=$(grep -A8 -E '^ +upload-assets:' .github/workflows/release.yml || true)

if echo "$UPLOAD_WINDOW" | grep -qE '^[[:space:]]+needs[[:space:]]?:[[:space:]]*$'; then
    ok "upload-assets has needs: block"
else
    bad "upload-assets is missing needs: block"
fi

for dep in release-please build-linux-deb build-mac-universal build-matrix-arch; do
    if echo "$UPLOAD_WINDOW" | grep -qE "^ +-+ +${dep}\$"; then
        ok "upload-assets needs ${dep}"
    else
        bad "upload-assets is missing needs: ${dep}"
    fi
done

# --- softprops/action-gh-release@v2 SHA-pinning rule (Phase 7 §7.7) ---
# Per the supply-chain-audit skill: NEVER reference the action by a
# mutable tag. The reference MUST be a 40-char commit SHA (or
# `owner/repo@<full-sha>`). We assert that the SHA is present AND
# that no `@v1` / `@v2` / `@v2.1` etc. tag-only reference is used.
echo
echo "=== softprops/action-gh-release SHA-pinning (supply-chain-audit) ==="
# Look for the SHA-pinned reference ONLY in actual `uses:` lines
# (where the action is invoked), not in free-text comments where
# `softprops/action-gh-release@v2` may appear as documentation.
# Pattern: line starts with `uses:`, has `softprops/action-gh-release@<40-hex>`
# Use a pattern that avoids the GNU grep 3.11 `:`+character-class
# quirk (see the needs-block check above).
SHA_PINNED=$(grep -E '^ +-? +uses:[[:space:]]+softprops/action-gh-release@[0-9a-f]{40}' .github/workflows/release.yml || true)
if [ -n "$SHA_PINNED" ]; then
    PINNED_SHA=$(echo "$SHA_PINNED" | grep -oE '[0-9a-f]{40}' | head -1)
    ok "softprops/action-gh-release is SHA-pinned: $PINNED_SHA"
else
    bad "softprops/action-gh-release is NOT SHA-pinned (must use a commit SHA, not a tag) on a uses: line"
fi

# Same scope rule for tag-form references — only flag if the tag
# reference is in an active `uses:` line. Free-text mentions of
# `@v2` in comments (e.g. "item 7-7 will wire v2 to ...") are
# documentation, not invocations.
TAG_REFERENCE=$(grep -E '^ +-? +uses:[[:space:]]+softprops/action-gh-release@(v[0-9]+(\.[0-9]+){0,2})([^0-9a-f]|$)' .github/workflows/release.yml || true)
if [ -n "$TAG_REFERENCE" ]; then
    bad "Found tag-form softprops/action-gh-release reference (must use SHA):"
    echo "$TAG_REFERENCE"
else
    ok "no active tag-form references to softprops/action-gh-release (comment mentions ok)"
fi

# --- draft-build act dry-run guard (Phase 7 §7.7) ---
# The act -n check above already covers draft-build.yml — explicit
# echo here so §7.7's checklist has a documented single source of
# truth. yamllint + act also run in the §7.1 block above.
echo
echo "=== draft-build act -n is covered by the [5] act -n block above ==="
note_warn "if act became installed since this script ran, rerun and check rc_db=0"

# --- Summary ---
echo
echo "=== Summary: pass=$PASS, warn=$WARN, fail=$FAIL ==="
if [ "$FAIL" -gt 0 ]; then
    echo "✗ Workflow smoke FAILED"
    exit 1
fi

echo "✓ Workflow smoke passed"
