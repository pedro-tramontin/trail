//! `notarize` — env-var self-test for the macOS code-signing + notarization
//! pipeline.
//!
//! Every Apple signing/notarization credential is read from the runner's
//! environment at `cargo tauri build` time. The `tauri.conf.json` bundle
//! block expands `${VAR}` placeholders at bundling, so the same config file
//! works locally (where the vars are unset → Tauri skips signing) and in
//! CI (where the GitHub Actions Secrets populate the env).
//!
//! This module exposes a `notarize_check` Tauri command that returns
//! a `BTreeMap<String, String>` of `env-var name → "set" | "unset"`. The
//! frontend (or a smoke harness) can call it before kicking off a build
//! to confirm the env is wired correctly.
//!
//! SECURITY: the map only carries `"set"` / `"unset"` strings — NEVER the
//! actual value. The macOS signing identity, the base64 cert, and the App
//! Store Connect API key are all sensitive. Echoing the value back through
//! the IPC channel would put it in any logs that capture IPC traffic
//! (e.g. Chrome DevTools' Tauri devtools panel) and create a heap-dump
//! leak surface. Always return the "is it set?" boolean, not the value.

use serde::Serialize;
use std::collections::BTreeMap;
use std::env;

/// Canonical set of env vars the macOS signing + notarization pipeline
/// reads. Order is stable so JSON output is byte-identical between
/// invocations (helps the smoke harness diff the output).
///
/// - `APPLE_SIGNING_IDENTITY` — the Developer ID Application identity
///   string (e.g. `"Developer ID Application: Pedro Tramontin (XYZ123)"`).
///   Read by `bundle.macOS.signingIdentity` in `tauri.conf.json`.
/// - `APPLE_TEAM_ID` — the 10-character Apple Developer Team ID. Read by
///   `bundle.macOS.providerShortName` in `tauri.conf.json` and by the
///   notary workflow's `--team` flag.
/// - `APPLE_IDENTITY_P8_KEY_PATH` — absolute path to the `.p8` App Store
///   Connect API key file. Notarization tool reads it from disk.
/// - `APPLE_IDENTITY_P8_KEY_ID` — the API key's `KEY_ID` (10-char).
/// - `APPLE_API_KEY_ID` — alternative API key ID form (some notarization
///   tools prefer this name).
/// - `APPLE_API_ISSUER_ID` — the API issuer (UUID form).
pub const NOTARIZE_ENV_VARS: &[&str] = &[
    "APPLE_SIGNING_IDENTITY",
    "APPLE_TEAM_ID",
    "APPLE_IDENTITY_P8_KEY_PATH",
    "APPLE_IDENTITY_P8_KEY_ID",
    "APPLE_API_KEY_ID",
    "APPLE_API_ISSUER_ID",
];

/// Result of `notarize_check`. The map is sorted (BTreeMap) so the JSON
/// output is deterministic — important for the smoke harness's diff
/// against a known-good baseline.
#[derive(Debug, Serialize)]
pub struct NotarizeEnvReport {
    /// env-var name → "set" / "unset".
    pub env: BTreeMap<String, String>,
}

/// Build the env-var report. Pure function; no IO besides the env read.
/// Public (not just `fn notarize_check`) so unit tests can call it
/// without spinning up a Tauri runtime.
pub fn check() -> NotarizeEnvReport {
    let mut env = BTreeMap::new();
    for var in NOTARIZE_ENV_VARS {
        // `var_os` returns `Some` for both set-with-value and set-to-empty
        // strings; the spec says "set" iff the var is set at all (the
        // empty-string case is treated as unset — matches Tauri behavior,
        // which also skips signing when the env-expanded string is empty).
        let status = if env::var_os(var).map(|v| !v.is_empty()).unwrap_or(false) {
            "set"
        } else {
            "unset"
        };
        env.insert((*var).to_string(), status.to_string());
    }
    NotarizeEnvReport { env }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // The Tauri command surface in lib.rs runs in a single-threaded
    // test process by default, but `std::env::set_var` is documented as
    // not thread-safe. The lock serializes the test cases below and
    // prevents an interleaved test (in another module) from clobbering
    // our env state mid-run. A poisoned mutex is fine — it just means a
    // panic happened in another test; the env-var reads themselves
    // don't care.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_all() {
        for v in NOTARIZE_ENV_VARS {
            // SAFETY: tests are serialized by ENV_LOCK.
            unsafe { env::remove_var(v) };
        }
    }

    /// With no env vars set, every entry in the report is `"unset"`
    /// and the map is sorted by env-var name (so the iteration order
    /// is stable).
    #[test]
    fn empty_env_marks_every_var_unset() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        let report = check();
        assert_eq!(report.env.len(), NOTARIZE_ENV_VARS.len());
        for (k, v) in &report.env {
            assert!(
                NOTARIZE_ENV_VARS.contains(&k.as_str()),
                "unexpected key in report: {k}"
            );
            assert_eq!(v, "unset", "var {k} should be unset, got {v}");
        }
        // Sorted invariant: BTreeMap iterates in key order, so the
        // first key is the alphabetically smallest env-var name.
        let first = report.env.keys().next().unwrap().clone();
        assert_eq!(first, "APPLE_API_ISSUER_ID");
    }

    /// With all six vars set to non-empty values, every entry is
    /// `"set"`. Confirms the map covers every name in the spec.
    #[test]
    fn all_six_set_marks_every_var_set() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        // SAFETY: tests are serialized by ENV_LOCK.
        unsafe {
            env::set_var(
                "APPLE_SIGNING_IDENTITY",
                "Developer ID Application: Pedro (XYZ123)",
            );
            env::set_var("APPLE_TEAM_ID", "XYZ1234567");
            env::set_var("APPLE_IDENTITY_P8_KEY_PATH", "/keys/AuthKey.p8");
            env::set_var("APPLE_IDENTITY_P8_KEY_ID", "ABCDE12345");
            env::set_var("APPLE_API_KEY_ID", "ABCDE12345");
            env::set_var(
                "APPLE_API_ISSUER_ID",
                "00000000-0000-0000-0000-000000000000",
            );
        }
        let report = check();
        assert_eq!(report.env.len(), NOTARIZE_ENV_VARS.len());
        for v in report.env.values() {
            assert_eq!(v, "set", "all six must report set, got {v}");
        }
        clear_all();
    }

    /// A var set to the empty string is reported as `"unset"`. The
    /// `var_os` API returns `Some(OsString::new())` in that case, and
    /// Tauri skips signing when the expanded value is empty — so
    /// treating the empty-string case as unset is the consistent
    /// contract across both the runtime check and the bundler.
    ///
    /// Also a security check: we never echo the value through IPC.
    /// The `notarize_check` Tauri command returns a `"set"` / `"unset"`
    /// string only — the actual bytes (e.g. the .p8 key path) never
    /// leave the Rust process via this command.
    #[test]
    fn empty_string_treated_as_unset_and_value_never_echoed() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        clear_all();
        // SAFETY: tests are serialized by ENV_LOCK.
        unsafe {
            env::set_var("APPLE_SIGNING_IDENTITY", "");
            // Sentinel value — if anything echoes the var's content
            // back, this substring would appear in the report.
            env::set_var("APPLE_TEAM_ID", "SHOULD-NOT-LEAK-IN-REPORT");
        }
        let report = check();
        assert_eq!(
            report.env.get("APPLE_SIGNING_IDENTITY").map(String::as_str),
            Some("unset"),
            "empty string must report unset"
        );
        assert_eq!(
            report.env.get("APPLE_TEAM_ID").map(String::as_str),
            Some("set"),
            "non-empty string must report set"
        );
        // Serialise to JSON and grep for the sentinel — it must NOT
        // appear. (The serde_json path is the one the IPC channel
        // would take.)
        let json = serde_json::to_string(&report).expect("serialise");
        assert!(
            !json.contains("SHOULD-NOT-LEAK-IN-REPORT"),
            "report leaked the var value through the JSON: {json}"
        );
        clear_all();
    }
}
