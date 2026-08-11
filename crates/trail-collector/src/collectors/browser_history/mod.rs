// SPDX-License-Identifier: MIT
//
// browser_history/mod.rs — the `browser_history` collector entry
// point + the per-browser dispatch. Submodule split:
//
//   * `chromium` — Chrome, Brave, Opera (all share the Chromium
//                  schema). One reader covers all three.
//   * `firefox`  — Firefox `places.sqlite`. Different schema
//                  (moz_places + moz_historyvisits), different
//                  timestamp encoding (Unix seconds vs Chromium's
//                  WebKit µs), different per-profile layout
//                  (random `xxxxxxxx.default-release` dirs).
//   * `safari`   — `History.db`. macOS-only; gated to
//                  `#[cfg(target_os = "macos")]`.
//
// Each submodule exposes `pub fn read_*` returning
// `Vec<RawHistoryRow>`; the top-level `run()` calls all three
// (filtered by the user's pick list from `Config.browser_history`)
// and folds them into one payload via
// `super::synth_browser_history::synthesize`.
//
// **Privacy posture (plan
// `.hermes/plans/2026-08-11_browser-history-collector.md` §D1):**
// capture every field the schema declares — `transition_type`,
// `typed_count`, `referrer_url` — and let the downstream LLM
// anonymizer scrub PII before the payload reaches the VPS.

use anyhow::Result;
use chrono::{Local, Utc};
use serde::{Deserialize, Serialize};

use super::synth_browser_history::{self, Browser, RawHistoryRow};
use super::RawOutput;

pub mod chromium;
pub mod firefox;
#[cfg(target_os = "macos")]
pub mod safari;

/// What the laptop's `Config` knows about the user's browser
/// pick. Mirrors the calendar `CalendarSourceChoice` shape — the
/// Tauri supervisor passes this into the collector subprocess via
/// `CollectorLaptopConfig::browser_history` (added in the
/// `BrowserHistoryLaptopConfig` section of `super::mod.rs`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserHistoryInput {
    /// Browsers the user picked in the wizard, intersected with
    /// the scanner's `Available` set. An empty `Vec` means "no
    /// browsers" — the supervisor still spawns the collector (so
    /// the daily review prompt can show "no browsing captured
    /// today"), and `run()` returns an empty payload.
    ///
    /// Serialised as the string list the user picked (e.g.
    /// `["chrome", "firefox"]`) — matches the wizard's
    /// `answers.browser_history` wire schema.
    #[serde(rename = "enabled_browsers")]
    pub enabled_browsers: Vec<Browser>,
    /// Resolved DB paths per browser (the scanner already computed
    /// these from the user's home + platform). The collector
    /// doesn't recompute — keeps a single source of truth.
    #[serde(rename = "db_paths")]
    pub db_paths: Vec<BrowserDbPath>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserDbPath {
    pub browser: Browser,
    pub path: std::path::PathBuf,
    pub profile: String,
}

/// Top-level entry: assemble rows from every picked browser,
/// synthesize the payload, wrap in the supervisor envelope.
///
/// `input.db_paths` is filtered against `input.enabled_browsers`
/// inside `run` — the supervisor passes the full scanner-evidence
/// list and we drop browsers the user didn't pick. The Firefox
/// reader handles its own per-profile glob (Firefox has multiple
/// profiles in one `profiles/` dir).
pub fn run(input: &BrowserHistoryInput) -> Result<RawOutput> {
    let now = Utc::now();
    let today = Local::now().date_naive();

    let mut all_rows: Vec<RawHistoryRow> = Vec::new();

    // Build the per-reader work list. Chromium readers get one
    // (browser, path, profile) tuple per (browser × profile)
    // combination; Firefox gets the union of all firefox paths;
    // Safari gets the macOS-only History.db if FDA is granted.
    let mut chromium_targets: Vec<(Browser, std::path::PathBuf, String)> = Vec::new();
    let mut firefox_paths: Vec<std::path::PathBuf> = Vec::new();
    // Safari is macOS-only. On non-macOS the variable is unused;
    // the `#[cfg]` gate below drops the assignment site too.
    #[cfg(target_os = "macos")]
    let mut safari_path: Option<std::path::PathBuf> = None;
    for db in &input.db_paths {
        match db.browser {
            Browser::Chrome | Browser::Brave | Browser::Opera => {
                if input.enabled_browsers.contains(&db.browser) {
                    chromium_targets.push((db.browser, db.path.clone(), db.profile.clone()));
                }
            }
            Browser::Firefox => {
                if input.enabled_browsers.contains(&Browser::Firefox) {
                    firefox_paths.push(db.path.clone());
                }
            }
            Browser::Safari => {
                if input.enabled_browsers.contains(&Browser::Safari) {
                    #[cfg(target_os = "macos")]
                    {
                        safari_path = Some(db.path.clone());
                    }
                }
            }
        }
    }

    if !chromium_targets.is_empty() {
        match chromium::read_all_chromium(&chromium_targets) {
            Ok(rows) => all_rows.extend(rows),
            Err(e) => tracing::warn!(error = %e, "chromium browser-history reader failed; skipping Chromium browsers"),
        }
    }

    if !firefox_paths.is_empty() {
        match firefox::read_all_firefox(&firefox_paths) {
            Ok(rows) => all_rows.extend(rows),
            Err(e) => tracing::warn!(error = %e, "firefox browser-history reader failed; skipping Firefox"),
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(path) = safari_path {
            match safari::read_safari(&path) {
                Ok(rows) => all_rows.extend(rows),
                Err(e) => tracing::warn!(error = %e, "safari reader failed; skipping Safari"),
            }
        }
    }

    let payload = synth_browser_history::synthesize(&all_rows, today, now)?;
    synth_browser_history::envelope(payload, today, now)
}

/// Helper for callers that want to produce an empty envelope
/// without a full `run` (e.g. when `input.enabled_browsers` is
/// empty but the supervisor still wants a valid envelope to
/// validate).
pub fn empty() -> Result<RawOutput> {
    let now = Utc::now();
    let today = Local::now().date_naive();
    synth_browser_history::empty_envelope(today, now)
}