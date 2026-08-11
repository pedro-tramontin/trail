// SPDX-License-Identifier: MIT
//
// calendar/mod.rs — the `calendar` collector entry point + the
// platform dispatch (EventKit on macOS, `.ics` file parser on Linux +
// macOS fallback). Submodule split:
//
//   * `ical`    — `.ics` file parser. Used on Linux (the VPS-shipped
//                 musl build) and as the macOS fallback when the
//                 user can't grant TCC to EventKit. Always compiles.
//   * `eventkit` — `EventKit.framework` reader via `objc2-event-kit`.
//                 macOS only. Gated to `target_os = "macos"` so the
//                 musl cross-compile never sees the `objc2-event-kit`
//                 dependency (which transitively links AppKit).
//
// The top-level `run` function dispatches to `ical::run` on non-macOS
// and the chosen source on macOS. The dispatch decision reads
// `CollectorLaptopConfig.calendar_source` (the `CalendarSourceChoice`
// tagged enum lives in `super` and lands in this same PR — see
// `mod.rs` in the parent `collectors/` module for the migration
// shim).
//
// **Privacy rule (Phase 2 §2.4 / design doc §2):** the synthesizer
// only emits `uid`, `summary`, `start`, `duration_minutes`,
// `attendees`, `organizer`, `location`, and (macOS EventKit only,
// post-anonymize) `notes`. The EventKit reader never asks for
// `EKEvent.description` / `EKEvent.comments` / `.ics DESCRIPTION` /
// `.ics COMMENT` / `.ics X-ALT-DESC`. The schema reflects this
// allowlist.

use anyhow::Result;

use super::{CollectorLaptopConfig, RawOutput};

pub mod ical;
#[cfg(target_os = "macos")]
pub mod eventkit;

/// Top-level entry: dispatch to the active source's backend. On macOS
/// the source may be `EventKit` (live read) or `Ics` (file path). On
/// Linux (and other non-macOS targets) only `Ics` is supported — the
/// supervisor turns a `EventKit` choice on Linux into a
/// config-validation error before reaching here.
///
/// `calendar_source` is a re-export of the `CalendarSourceChoice`
/// enum from `super` so call sites in `dispatch` (the parent
/// `collectors/mod.rs`) can stay agnostic to the calendar-specific
/// type.
pub fn run(cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    match cfg.calendar_source {
        super::CalendarSourceChoice::Ics => ical::run(cfg),
        #[cfg(target_os = "macos")]
        super::CalendarSourceChoice::EventKit => eventkit::run(cfg),
        // The non-macOS arm. The `Config::validate` path on the Tauri
        // side rejects a Linux user who picked EventKit before this
        // collector is ever spawned, so the `unreachable!` is
        // defensive — if it ever fires, a misconfigured Tauri side
        // has been updated to allow EventKit on Linux. The
        // `#[allow(unreachable_patterns)]` is the matching arm's
        // permission to be present (it's a no-op on macOS, where
        // the `#[cfg]` arm above covers the variant).
        #[cfg(not(target_os = "macos"))]
        super::CalendarSourceChoice::EventKit => {
            unreachable!(
                "Config::validate on the Tauri side must reject `EventKit` \
                 on non-macOS before the collector subprocess is spawned. \
                 If this is reached, the validation gate regressed."
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::synth_calendar;
    use chrono::{NaiveDate, Utc};
    use serde_json::Value;

    // Fixtures and the bundled schema are read at compile time; the
    // non-test build carries no fixture bytes. The path is one
    // directory deeper than the old `collectors/calendar.rs` file —
    // we walk `../../../` (up out of `calendar/`, out of
    // `collectors/`, out of `src/`) before dropping into `schemas/`
    // and `tests/fixtures/`.
    const SCHEMA: &str = include_str!("../../../schemas/calendar.schema.json");
    const ICS_FIXTURE: &str = include_str!("../../../tests/fixtures/calendar/workday.ics");

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
    /// `compile_schema` will check at runtime; if it passes here, the
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
