//! Demo mode first-run experience.
//!
//! When the user launches Trail with the `--demo` flag AND no
//! `~/.trail/config.json` exists, the app starts in "demo mode":
//!
//!   * The dashboard / review window renders fixture data
//!     (`resources/fixtures/demo-day-summary.json`) instead of the
//!     real collector output.
//!   * A yellow banner at the top of every window reads the
//!     `DEMO_BANNER_TEXT` contract string so the user knows no
//!     real captures are happening.
//!   * Any "push to VPS" affordance in the UI is disabled — the
//!     fixture data never leaves the laptop.
//!
//! The bootstrap is two-condition (demo flag + no config on disk)
//! so a user with a real config who accidentally passes `--demo`
//! is NOT switched into fixture mode. The env-var handoff from
//! `main.rs` is the only state this module reads at runtime.
//!
//! Spec: Phase 7 §7.5.

use clap::Parser;
use serde::Serialize;
use std::path::{Path, PathBuf};

/// The literal banner string. Tracked here as a const so the
/// Svelte tests + the Rust tests can both grep for it. The
/// front-end `<DemoBanner />` component hard-codes the same
/// string in its template; this is the contract between the two.
pub const DEMO_BANNER_TEXT: &str =
    "Demo mode — no real captures. Go to Settings to set up real captures.";

/// Mirror of the binary's `Args` struct (see `src/main.rs`). Lives
/// here so the library crate can parse argv in unit tests without
/// depending on the binary. The two structs share the same
/// `TRAIL_DEMO=1` env-var handoff: `main.rs` parses argv and
/// `set_var`s, `lib::run()` calls `activate_if_requested` which
/// reads the same env var.
#[derive(Parser, Debug, Clone)]
#[command(
    name = "trail",
    version,
    about = "Trail menu-bar app — passive workday capture + VPS push."
)]
pub struct Args {
    /// Start the app in demo mode. If a real `~/.trail/config.json`
    /// already exists, this flag is ignored.
    #[arg(long)]
    pub demo: bool,
}

/// Fixture day-summary baked at compile time from the workspace
/// `resources/` directory. Mirrors the production schema
/// (`day-summary.schema.json` from Phase 1 §1.9) so the validator
/// can run against it unchanged.
pub const FIXTURE_DAY_SUMMARY: &str =
    include_str!("../../resources/fixtures/demo-day-summary.json");

/// Errors from the demo bootstrap. Currently the only failure mode
/// is "fixture JSON is malformed" — that would be a build-time
/// bug, but the function returns `Result` so the caller can
/// surface it without panicking.
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error("parsing bundled demo-day-summary.json fixture: {0}")]
    FixtureParse(#[source] serde_json::Error),
}

/// The active demo state, shared with the Svelte frontend via
/// `app.manage(DemoState)` and surfaced to Svelte through the
/// `demo_status` Tauri command (see `src/lib.rs`).
#[derive(Debug, Serialize, Clone)]
pub struct DemoState {
    /// True when demo mode is active for this launch.
    pub active: bool,
    /// The literal banner text the Svelte `<DemoBanner />`
    /// component renders. Same value as the const but
    /// serialised so the frontend doesn't need a hard-coded
    /// string (the Rust side is the source of truth).
    pub banner_text: &'static str,
    /// The fixture DaySummary, parsed and ready for the Review
    /// window. `None` when demo is not active.
    pub fixture_summary: Option<serde_json::Value>,
}

/// Compute the path to the user's Trail config directory. Uses
/// `dirs::home_dir()` so the test fixtures and the real
/// `resolve_paths` agree on the same `~/.trail/` location on
/// macOS / Linux.
pub fn config_dir() -> PathBuf {
    dirs::home_dir()
        .map(|h| h.join(".trail"))
        .unwrap_or_else(|| PathBuf::from(".trail"))
}

/// Decide whether demo mode should activate. Returns true iff:
///
///   1. `args.demo` is true (caller passed the `--demo` flag), AND
///   2. `~/.trail/config.json` is missing (first-run condition).
///
/// The two-condition check is critical: if the user has a real
/// config and accidentally passes `--demo`, we do NOT switch them
/// into fixture data. Symmetrically, if they pass the flag and
/// have no config, demo mode is active and the Svelte side reads
/// `DEMO_BANNER_TEXT` from the Tauri-managed `DemoState`.
pub fn should_activate(args: &Args, config_dir: &Path) -> bool {
    if !args.demo {
        return false;
    }
    let config_path = config_dir.join("config.json");
    !config_path.exists()
}

/// Parse the bundled fixture JSON into a `serde_json::Value` that
/// the Svelte side can hand to `<Review />` directly. Returns
/// the inner `serde_json::Error` (not `anyhow::Error`) so the
/// `DemoError::FixtureParse` `#[source]` chain carries the
/// underlying serde error verbatim.
pub fn fixture_day_summary() -> std::result::Result<serde_json::Value, serde_json::Error> {
    serde_json::from_str(FIXTURE_DAY_SUMMARY)
}

/// Entry point called from `lib::run()`. Computes the demo
/// state and returns it (or `None` when demo is not active so
/// the caller can choose whether to manage state at all).
///
/// The `Option` return is the spec's API: `Some(DemoState)` when
/// active, `None` when not. The `Result` wrapper surfaces the
/// fixture-parse failure path — in practice the fixture is
/// `include_str!`-baked so a parse failure is a build-time bug.
pub fn activate_if_requested(args: &Args) -> Result<Option<DemoState>, DemoError> {
    let dir = config_dir();
    let active = should_activate(args, &dir);
    if active {
        tracing::info!(
            "Demo mode activated (TRAIL_DEMO=1, no config at {})",
            dir.display()
        );
    }
    let state = if active {
        let summary = fixture_day_summary().map_err(DemoError::FixtureParse)?;
        Some(DemoState {
            active: true,
            banner_text: DEMO_BANNER_TEXT,
            fixture_summary: Some(summary),
        })
    } else {
        None
    };
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn banner_text_matches_contract() {
        assert_eq!(
            DEMO_BANNER_TEXT,
            "Demo mode — no real captures. Go to Settings to set up real captures."
        );
    }

    #[test]
    fn clap_parses_demo_flag() {
        // clap-derived Args parsing — proves the `Args` struct
        // accepts `--demo` and defaults to `false`.
        let parsed = Args::try_parse_from(["trail", "--demo"]).expect("parse --demo");
        assert!(parsed.demo, "--demo flag must be set to true");

        let parsed = Args::try_parse_from(["trail"]).expect("parse bare argv");
        assert!(!parsed.demo, "bare argv must default demo to false");
    }

    #[test]
    fn demo_flag_without_config_activates() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // The tempdir is empty (no config.json), so demo should activate.
        let args = Args { demo: true };
        assert!(
            should_activate(&args, tmp.path()),
            "demo flag + missing config must activate demo mode"
        );
    }

    #[test]
    fn demo_flag_with_existing_config_does_not_activate() {
        let tmp = tempfile::tempdir().expect("tempdir");
        fs::write(tmp.path().join("config.json"), "{}").expect("write config");
        let args = Args { demo: true };
        assert!(
            !should_activate(&args, tmp.path()),
            "existing config must block demo mode even when --demo is set"
        );
    }

    #[test]
    fn no_demo_flag_never_activates() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let args = Args { demo: false };
        assert!(
            !should_activate(&args, tmp.path()),
            "no --demo flag must never activate demo mode"
        );
    }

    #[test]
    fn fixture_day_summary_parses_and_has_all_required_keys() {
        let v = fixture_day_summary().expect("fixture parses as JSON");
        // Required keys per day-summary.schema.json (Phase 1 §1.9).
        for k in [
            "date",
            "summary",
            "wins",
            "blockers",
            "people",
            "open_threads",
            "voice_notes",
        ] {
            assert!(v.get(k).is_some(), "fixture missing required key: {k}");
        }
    }

    #[test]
    fn activate_if_requested_returns_some_when_active() {
        let tmp = tempfile::tempdir().expect("tempdir");
        // Force config_dir() to a known empty path by pointing
        // HOME at the tempdir. (config_dir() uses dirs::home_dir().)
        let prev_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        let args = Args { demo: true };
        let state = activate_if_requested(&args)
            .expect("activate_if_requested ok")
            .expect("state is Some when active");
        assert!(state.active);
        assert_eq!(state.banner_text, DEMO_BANNER_TEXT);
        let summary = state.fixture_summary.expect("fixture_summary present");
        assert!(summary.get("date").is_some());

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn activate_if_requested_returns_none_when_inactive() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", tmp.path());

        // No --demo flag → must return None.
        let args = Args { demo: false };
        let state = activate_if_requested(&args).expect("ok");
        assert!(state.is_none(), "no flag means no demo state");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
