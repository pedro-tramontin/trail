// SPDX-License-Identifier: MIT
//
// calendar/ical.rs — the `.ics` file parser backend for the calendar
// collector. Phase 2 §2.4. Owns the I/O (reading the configured `.ics`
// file from disk); the pure ICS→payload transform lives in
// `super::super::synth_calendar` so the synthesis step is unit-testable
// without any on-disk fixtures.
//
// The collector stays sync (a few ms for a personal calendar export);
// the Tauri orchestrator (§2.5) wraps it in
// `tokio::process::Command` if it needs to invoke this from an async
// context.
//
// **Path discovery:** the orchestrator reads
// `~/.trail/config.json::calendar.ics.path` and threads the path
// through `CollectorLaptopConfig.calendar_ics`. If the file doesn't
// exist, this collector bails with a clear error — the supervisor
// turns that into a non-zero exit and the Settings UI (§2.6) shows
// the missing-path state.
//
// **Privacy rule (Phase 2 §2.4 / design doc §2):** the synthesizer
// only emits `uid`, `summary`, `start`, `duration_minutes`,
// `attendees`, `organizer`, `location`. `DESCRIPTION`, `COMMENT`, and
// `X-ALT-DESC` are NEVER captured — calendar event bodies frequently
// leak meeting context, customer names, or healthcare details. See
// `super::super::synth_calendar` for the allowlist tokenizer.

use anyhow::{Context, Result};
use chrono::{Local, Utc};

use super::super::synth_calendar;
use super::super::{CollectorLaptopConfig, RawOutput};

/// Read the configured `.ics`, extract today's events, return the
/// supervisor-validated envelope.
///
/// Bails with a clear error if the file isn't present (a fresh laptop
/// without a Calendar export configured will see this).
pub fn run(cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    let path = &cfg.calendar_ics;
    if !path.exists() {
        anyhow::bail!("calendar .ics not found at {}", path.display());
    }
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let now = Utc::now();
    let today = Local::now().date_naive();
    let payload =
        synth_calendar::synthesize(&text, today, now).context("synthesizing calendar payload")?;
    Ok(RawOutput {
        source: "calendar".to_string(),
        captured_at: now,
        date: today,
        payload,
    })
}
