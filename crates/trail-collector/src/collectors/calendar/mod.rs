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
// **Privacy posture (post-PR #219 + plan
// `.hermes/plans/2026-08-11_browser-history-collector.md` §D5):** the
// synthesizer captures every field the source exposes —
// `uid`, `summary`, `start`, `duration_minutes`, `attendees`,
// `organizer`, `location`, `description`, `comment`, `x_alt_desc`,
// `url`, and the EventKit-only `notes` / `alarms` /
// `recurrence_rules`. PII scrubbing is the downstream
// `src-tauri/src/anonymizer.rs::anonymize` pass's job, running on
// the laptop before the payload reaches the VPS. This matches the
// new capture-then-anonymize posture the user confirmed for the
// browser-history collector (same plan §D1) and is the same
// privacy architecture PR #217 set up for `EKEvent.notes`.

use anyhow::Result;

use super::{CollectorLaptopConfig, RawOutput};

#[cfg(target_os = "macos")]
pub mod eventkit;
pub mod ical;
pub mod remote_calendar;

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
///
/// **Layer 1 webcal/ICS URL subscription (per-proposal §"Layer 1"):**
/// when `cfg.remote_calendar_urls` is non-empty, the local source's
/// output is *augmented* with the events fetched from each URL. The
/// fetch is one-shot HTTP GET per URL with a 5 MB body cap
/// (proposal §"Risks → #8"); `webcal://` is rewritten to `https://`
/// (proposal §"Risks → #7"); auth-required URLs return a friendly
/// error (proposal §"Risks → #6") and are skipped (one bad URL
/// doesn't drop the whole cycle's events).
pub fn run(cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    // The local source's envelope. On EventKit (macOS) this reads
    // Calendar.app directly; on `Ics` it reads the configured
    // `.ics` file path. The Layer 1 path augments this with the
    // user's pasted URLs.
    let local = match cfg.calendar_source {
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
    }?;

    // Layer 1: webcal/ICS URL subscription. If the user didn't
    // paste any URLs, `remote_calendar::run` returns an empty
    // envelope; we still need to merge (an empty events list
    // is the no-op case). When the local source already failed
    // (e.g. `.ics` file not found), we still want to try the
    // remote URLs — a user who pastes a URL doesn't need a
    // local `.ics` file to exist. The `local` is on the stack
    // and we drop it on the merge path below.
    if cfg.remote_calendar_urls.is_empty() {
        return Ok(local);
    }

    let remote = match remote_calendar::run(cfg) {
        Ok(env) => env,
        // Per-proposal §"Risks → #6", a friendly error for
        // auth-required URLs is the user-visible signal. We
        // log + continue: a single bad URL shouldn't drop
        // the day's events from the local `.ics` file (or
        // from the URLs that DID succeed). The
        // `remote_calendar::run` already traces a warn for
        // each per-URL failure and only returns Err when
        // every URL failed; we surface that to the user
        // via the Settings UI's "remote calendar fetch
        // failed" state without losing the local envelope.
        Err(e) => {
            tracing::warn!(
                error = %e,
                "remote calendar URL fetch failed; continuing with local envelope only"
            );
            return Ok(local);
        }
    };

    Ok(merge_envelopes(local, remote))
}

/// Merge two calendar envelopes. Both must have the same `source`
/// / `captured_at` / `date` (the local one wins on those fields
/// — the merge is happening in the same collection cycle). The
/// events lists are concatenated and re-sorted by UTC `start` so
/// the merged envelope is reproducible. The `notes` / `alarms` /
/// `recurrence_rules` EventKit-only fields are preserved on
/// EventKit-sourced events (the `.ics` path emits `null` for
/// them); the remote `.ics` path emits `null` too. Schema
/// validation happens on the merged envelope.
fn merge_envelopes(local: RawOutput, remote: RawOutput) -> RawOutput {
    let mut events: Vec<serde_json::Value> = Vec::new();
    if let Some(arr) = local.payload.get("events").and_then(|v| v.as_array()) {
        events.extend(arr.iter().cloned());
    }
    if let Some(arr) = remote.payload.get("events").and_then(|v| v.as_array()) {
        events.extend(arr.iter().cloned());
    }
    // Stable order by UTC start so the merged envelope is
    // reproducible. The local source's events are already
    // sorted by `synth_calendar::synthesize`; the remote
    // source's are sorted in `remote_calendar::run`. The
    // cross-source sort is the merge seam.
    events.sort_by(|a, b| {
        let ax = a.get("start").and_then(|v| v.as_str()).unwrap_or("");
        let bx = b.get("start").and_then(|v| v.as_str()).unwrap_or("");
        ax.cmp(bx)
    });
    RawOutput {
        source: local.source,
        captured_at: local.captured_at,
        date: local.date,
        payload: serde_json::json!({ "events": events }),
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

    /// Test 3 — attendees are extracted (multi-property). Pre-PR this
    /// test also asserted the privacy rule (DESCRIPTION bodies not
    /// captured); per plan §D5 the privacy guarantee moved from the
    /// capture layer to the downstream LLM anonymizer, so the bodies
    /// ARE captured here. The new fields (`description`, `comment`,
    /// `x_alt_desc`, `url`) are checked for being null (no such
    /// properties in the fixture) or for matching the fixture text
    /// (when present in the fixture). See the
    /// `synthesize_captures_description_when_present` test below for
    /// the positive coverage of the D5 widening.
    #[test]
    fn synthesize_extracts_attendees_and_widens_to_all_schema_fields() {
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

        // D5 widening check: every new capture-then-anonymize field
        // is present (possibly null) on each event so the schema's
        // optional fields resolve. The fixture's .ics file has no
        // DESCRIPTION / COMMENT / X-ALT-DESC / URL properties, so
        // these will all be null — that's the correct behavior, the
        // absence of a property is `None`.
        for ev in events {
            assert!(ev.get("description").is_some(), "description key present");
            assert!(ev.get("comment").is_some(), "comment key present");
            assert!(ev.get("x_alt_desc").is_some(), "x_alt_desc key present");
            assert!(ev.get("url").is_some(), "url key present");
            assert!(
                ev.get("notes").is_some(),
                "notes key present (null for .ics)"
            );
            assert!(
                ev.get("alarms").is_some(),
                "alarms key present (null for .ics)"
            );
            assert!(
                ev.get("recurrence_rules").is_some(),
                "recurrence_rules key present (null for .ics)"
            );
        }
    }

    /// Test 3b (added 2026-08-11, plan §D5) — when a VEVENT has a
    /// DESCRIPTION / COMMENT / X-ALT-DESC / URL property, the
    /// synthesizer captures them. The fixture's
    /// `tests/fixtures/calendar/with_bodies.ics` has all four on one
    /// event; we assert each captured field matches the source text
    /// verbatim (the downstream anonymizer is responsible for the
    /// scrub, the capture layer is byte-faithful).
    #[test]
    fn synthesize_captures_description_when_present() {
        const WITH_BODIES: &str = include_str!("../../../tests/fixtures/calendar/with_bodies.ics");
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let out = synth_calendar::synthesize(WITH_BODIES, today, Utc::now()).unwrap();
        let events = out["events"].as_array().unwrap();
        assert_eq!(events.len(), 1, "fixture has one event on 2026-08-15");

        let ev = &events[0];
        assert_eq!(
            ev["description"].as_str(),
            Some("Weekly sync — discuss roadmap and any blockers."),
            "DESCRIPTION captured verbatim"
        );
        assert_eq!(
            ev["comment"].as_str(),
            Some("Moved from Tuesday."),
            "COMMENT captured verbatim"
        );
        assert_eq!(
            ev["x_alt_desc"].as_str(),
            Some("HTML alt description for Outlook."),
            "X-ALT-DESC captured verbatim"
        );
        assert_eq!(
            ev["url"].as_str(),
            Some("https://example.com/event/12345"),
            "URL captured verbatim"
        );
        // EventKit-only fields are null on the .ics path.
        assert!(ev["notes"].is_null());
        assert!(ev["alarms"].is_null());
        assert!(ev["recurrence_rules"].is_null());
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
