#!/usr/bin/env bash
# tests/workflows_smoke.sh
#
# Validates the GH Actions workflow files in this repo. Run from the
# repo root: `bash tests/workflows_smoke.sh`.
#
# Cases:
#   1. release-please-config.json is valid JSON (jq parse).
#   2. .release-please-manifest.json is valid JSON (jq parse).
#   3. .github/workflows/{release,draft-build}.yml are valid YAML.
#   4. yamllint exits 0 for both workflow files (if yamllint installed).
#   5. act -n exits 0 for both workflow files (if act installed).
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

# --- Case 1: release-please-config.json ---
echo "[1] release-please-config.json JSON validity"
if command -v jq >/dev/null 2>&1; then
    if jq -e . release-please-config.json >/dev/null; then
        ok "release-please-config.json is valid JSON"
    else
        bad "release-please-config.json is not valid JSON"
    fi
else
    python3 -c "import json,sys; json.load(open('release-please-config.json'))" \
        && ok "release-please-config.json is valid JSON (python fallback)" \
        || bad "release-please-config.json is not valid JSON"
fi

# --- Case 2: .release-please-manifest.json ---
echo
echo "[2] .release-please-manifest.json JSON validity"
if command -v jq >/dev/null 2>&1; then
    if jq -e . .release-please-manifest.json >/dev/null; then
        ok ".release-please-manifest.json is valid JSON"
    else
        bad ".release-please-manifest.json is not valid JSON"
    fi
else
    python3 -c "import json,sys; json.load(open('.release-please-manifest.json'))" \
        && ok ".release-please-manifest.json is valid JSON (python fallback)" \
        || bad ".release-please-manifest.json is not valid JSON"
fi

# --- Case 3: YAML structural validity ---
echo
echo "[3] GitHub Actions YAML structural validity"
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

# --- Case 4: yamllint (optional) ---
echo
echo "[4] yamllint (optional)"
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

# --- Case 5: act -n dry-run (optional) ---
echo
echo "[5] act -n dry-run (optional)"
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

# --- Version-sync check ---
echo
echo "=== Version sync: Cargo.toml <-> release-please manifest ==="
ROOT_VER=$(grep '^version = ' Cargo.toml | head -1 | cut -d'"' -f2)
if command -v jq >/dev/null 2>&1; then
    MANIFEST_VER=$(jq -r '."."' .release-please-manifest.json)
else
    MANIFEST_VER=$(python3 -c "import json; print(json.load(open('.release-please-manifest.json'))['.'])")
fi
if [ "$ROOT_VER" = "$MANIFEST_VER" ]; then
    ok "version-sync: Cargo.toml=$ROOT_VER, manifest=$MANIFEST_VER"
else
    bad "Cargo.toml version ($ROOT_VER) != manifest version ($MANIFEST_VER)"
fi

# --- Per-crate version invariant (Phase 7 §7.2 — src-tauri only) ---
# release-please can't bump workspace-inherited versions (it walks
# `extraFiles` in release-please-config.json and only handles inline
# `version = "X.Y.Z"`). Per talon issue #2111 every workspace member
# must declare an inline version. Item 7-2 enforces the src-tauri
# crate; the trail-collector crate's equivalent is item 7-3's scope
# (the universal-binary build) — not gated here so the smoke passes
# incrementally as items land.
echo
echo "=== Per-crate version invariant (no version.workspace = true) ==="
WORKSPACE_VERSION_CRATES=0
for crate_toml in src-tauri/Cargo.toml; do
    if [ -f "$crate_toml" ]; then
        hits=$(grep -c '^version\.workspace *= *true' "$crate_toml" || true)
        if [ "$hits" -gt 0 ]; then
            bad "$crate_toml uses version.workspace = true (per-crate inline required)"
            WORKSPACE_VERSION_CRATES=$((WORKSPACE_VERSION_CRATES + 1))
        fi
    fi
done
if [ "$WORKSPACE_VERSION_CRATES" -eq 0 ]; then
    ok "src-tauri/Cargo.toml uses inline version (not workspace-inherited)"
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

# --- Summary ---
echo
echo "=== Summary: pass=$PASS, warn=$WARN, fail=$FAIL ==="
if [ "$FAIL" -gt 0 ]; then
    echo "✗ Workflow smoke FAILED"
    exit 1
fi

echo "✓ Workflow smoke passed"
