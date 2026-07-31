// SPDX-License-Identifier: MIT
//
// calendar.rs — the `calendar` collector entry point + its 4 spec tests.
//
// Phase 2 §2.4. Owns the I/O (reading the configured `.ics` file from
// disk); the pure ICS→payload transform lives in `synth_calendar.rs`
// next door so the synthesis step is unit-testable without any on-disk
// fixtures. The collector stays sync (a few ms for a personal calendar
// export); the Tauri orchestrator (§2.5) wraps it in
// `tokio::process::Command` if it needs to invoke this from an async
// context.
//
// **Path discovery:** the orchestrator reads
// `~/.trail/config.json::calendar_ics` and threads the path through
// `CollectorLaptopConfig.calendar_ics`. If the file doesn't exist, this
// collector bails with a clear error — the supervisor turns that into a
// non-zero exit and the Settings UI (§2.6) shows the missing-path state.
//
// **Privacy rule (Phase 2 §2.4 / design doc §2):** the synthesizer
// only emits `uid`, `summary`, `start`, `duration_minutes`, `attendees`,
// `organizer`, `location`. `DESCRIPTION`, `COMMENT`, and `X-ALT-DESC`
// are NEVER captured — calendar event bodies frequently leak meeting
// context, customer names, or healthcare details. See
// `synth_calendar.rs` for the allowlist tokenizer.

use super::synth_calendar;
use super::{CollectorLaptopConfig, RawOutput};
use anyhow::{Context, Result};
use chrono::{Local, Utc};

/// Top-level entry: read the configured `.ics`, extract today's
/// events, return the supervisor-validated envelope.
///
/// Bails with a clear error if the file isn't present (a fresh
/// laptop without a Calendar export configured will see this).
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use serde_json::Value;

    // Fixtures and the bundled schema are read at compile time; the
    // non-test build carries no fixture bytes.
    const SCHEMA: &str = include_str!("../../schemas/calendar.schema.json");
    const ICS_FIXTURE: &str = include_str!("../../tests/fixtures/calendar/workday.ics");

    /// Test 1 — today-only filter. The fixture has three events: two on
    /// 2026-07-31 and one on 2026-07-25. Calling `synthesize` with a
    /// `today` of 2026-07-31 returns exactly the two today events; a
    /// mismatched `today` returns zero.
    #[test]
    fn synthesize_filters_to_today_only() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let out = synth_calendar::synthesize(ICS_FIXTURE, today, Utc::now()).unwrap();
        let events = out["events"].as_array().unwrap();
        assert_eq!(
            events.len(),
            2,
            "fixture has 2 events on 2026-07-31; expected exactly 2 events"
        );

        // Boundary: a different `today` drops everything.
        let other_day = NaiveDate::from_ymd_opt(2026, 7, 25).unwrap();
        let out2 = synth_calendar::synthesize(ICS_FIXTURE, other_day, Utc::now()).unwrap();
        assert_eq!(out2["events"].as_array().unwrap().len(), 1);

        let unrelated_day = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
        let out3 = synth_calendar::synthesize(ICS_FIXTURE, unrelated_day, Utc::now()).unwrap();
        assert_eq!(out3["events"].as_array().unwrap().len(), 0);
    }

    /// Test 2 — `duration_minutes` computed from `DTEND - DTSTART` in
    /// the UTC timezone. Fixture event 1 is 10:00–11:00Z (60 min) and
    /// event 2 is 16:00–16:30Z (30 min). Output is sorted by start so
    /// event 1 (60 min) is index 0 and event 2 (30 min) is index 1.
    #[test]
    fn synthesize_computes_duration_in_minutes() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let out = synth_calendar::synthesize(ICS_FIXTURE, today, Utc::now()).unwrap();
        let events = out["events"].as_array().unwrap();
        assert_eq!(events[0]["duration_minutes"], 60, "10:00–11:00Z = 60 min");
        assert_eq!(events[1]["duration_minutes"], 30, "16:00–16:30Z = 30 min");
    }

    /// Test 3 — attendees are extracted (multi-property) and the
    /// privacy rule holds: `DESCRIPTION` content from each event's ICS
    /// body is NOT present in the resulting JSON.
    #[test]
    fn synthesize_extracts_attendees_and_does_not_capture_body() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let out = synth_calendar::synthesize(ICS_FIXTURE, today, Utc::now()).unwrap();
        let events = out["events"].as_array().unwrap();

        // Event 1 (Design review with Alice) has Alice + Bob.
        assert_eq!(
            events[0]["attendees"].as_array().unwrap().len(),
            2,
            "fixture event 1 has 2 attendees"
        );
        // Event 2 (1:1 with manager) has Pedro (only organizer counts + 1 ATTENDEE).
        assert_eq!(
            events[1]["attendees"].as_array().unwrap().len(),
            1,
            "fixture event 2 has 1 attendee"
        );

        // Privacy rule — serialize each event and assert that the bodies
        // ("Discuss the wizard variants", "Discuss career goals and
        // feedback") never appear in the raw output, nor do the
        // case-insensitive substrings of either.
        let payload_str = serde_json::to_string(&out).unwrap();
        assert!(
            !payload_str.contains("Discuss the wizard variants"),
            "DESCRIPTION body must NOT leak: got {payload_str}"
        );
        assert!(
            !payload_str.contains("Discuss career goals"),
            "DESCRIPTION body must NOT leak: got {payload_str}"
        );
        assert!(
            !payload_str.to_lowercase().contains("wizard"),
            "case-insensitive DESCRIPTION leak check: got {payload_str}"
        );
        assert!(
            !payload_str.contains("Should not appear"),
            "filtered event's DESCRIPTION body must NOT appear either: got {payload_str}"
        );
    }

    /// Test 4 — the payload validates against the bundled schema
    /// (Draft 2020-12). This is the same shape the supervisor's
    /// compile_schema will check at runtime; if it passes here, the
    /// `run()` → `RawOutput` → schema round-trip is honest.
    #[test]
    fn synthesize_payload_validates_against_schema() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let payload = synth_calendar::synthesize(ICS_FIXTURE, today, Utc::now()).unwrap();

        // Schema validation runs against the full envelope (the same
        // shape the supervisor wraps `payload` in via `RawOutput`),
        // because the schema's required root fields include
        // `source` / `captured_at` / `date` which we synthesise in
        // `run()`.
        let envelope = serde_json::json!({
            "source":      "calendar",
            "captured_at": Utc::now().to_rfc3339(),
            "date":        today.format("%Y-%m-%d").to_string(),
            "payload":     payload,
        });

        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let errors: Option<Vec<String>> = {
            let compiled = jsonschema::JSONSchema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .compile(&schema)
                .unwrap();
            compiled
                .validate(&envelope)
                .err()
                .map(|it| it.map(|e| e.to_string()).collect::<Vec<_>>())
        };
        if let Some(errs) = errors {
            for m in &errs {
                eprintln!("schema error: {m}");
            }
            panic!("envelope failed schema validation: {} error(s)", errs.len());
        }
    }
}
