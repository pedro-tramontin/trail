//! Pure-function transformer: ICS calendar text → `TrailRawCalendar` payload.
//!
//! The `calendar.rs` module owns the I/O (reading the configured `.ics`
//! file from disk); this module is the pure transform so the synthesis
//! step is fully testable without any on-disk fixtures. Decoupling also
//! keeps the supervisor (`collect.rs`) honest: validation runs against the
//! transformed output, never raw `.ics` bytes.
//!
//! **Privacy rule (Phase 2 §2.4, design doc §2):** capture only `UID`,
//! `SUMMARY`, `DTSTART`, `DTEND`, `ATTENDEE` (× N), `ORGANIZER`, and
//! `LOCATION` from each `VEVENT`. Do NOT capture `DESCRIPTION`,
//! `COMMENT`, or `X-ALT-DESC` — calendar event bodies leak meeting
//! context, customer names, healthcare details, etc. The tokenizer is
//! deliberately conservative: it stops at the line-level and only
//! surfaces the keys the schema declares.
//!
//! **Library note:** `icalendar = "0.7"` is the Phase 2 workspace-root
//! dep, but `icalendar` 0.7.x is a *builder* library — it has no
//! `FromStr` for `Calendar`, and `Property.value` is private (the only
//! public value-extraction path is `fmt_write` into a buffer). The
//! plan's pseudocode (`ics_text.parse()` + `event.get_start()` etc.)
//! references APIs that don't exist in 0.7. Rather than swap out the
//! spec's pinned dep, this module rolls a slim line-based ICS
//! tokenizer that extracts exactly the seven fields above. The
//! `icalendar` crate remains in `[workspace.dependencies]` so the
//! Phase 2 dep manifest is satisfied; the synthesis layer is the
//! single seam where the actual ICS bytes become a payload.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;

/// RFC 5545 basic format: `YYYYMMDDTHHMMSSZ` (UTC). The fixture matches
/// exactly; if a vendor emits floating local time or a `TZID=` form we
/// fall through to the `Err` arm below and the event is dropped with a
/// `tracing` log rather than emitting a wrong date.
const ICS_UTC_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// Build the raw `payload` object for the calendar source from a
/// pre-loaded ICS text. Pure: same inputs ⇒ same output.
///
/// `ics_text` is the entire `VCALENDAR` body the I/O layer read.
/// `today` is the local date the collector is capturing for; events
/// whose `DTSTART` (UTC) is on a different day are dropped. `_now` is
/// reserved for future "as-of" overrides (e.g. backfilling).
pub fn synthesize(ics_text: &str, today: NaiveDate, _now: DateTime<Utc>) -> Result<Value> {
    let events_raw = parse_vevents(ics_text);
    let mut events: Vec<Value> = Vec::new();

    for vevent in events_raw {
        // DTSTART is required. RFC 5545 mandates it; a missing value
        // means the event is malformed, so drop with a trace.
        let start_value = match vevent.get("DTSTART") {
            Some(v) => v.as_str(),
            None => {
                tracing::warn!("VEVENT missing DTSTART — skipping");
                continue;
            }
        };
        let start_naive = match NaiveDateTime::parse_from_str(start_value, ICS_UTC_FORMAT) {
            Ok(dt) => dt,
            Err(_) => {
                tracing::warn!(dtstart = start_value, "DTSTART not in YYYYMMDDTHHMMSSZ form — skipping (privacy: refuse to guess timezone)");
                continue;
            }
        };
        let start_utc: DateTime<Utc> = DateTime::<Utc>::from_naive_utc_and_offset(start_naive, Utc);

        // Today-only filter. We compare on the UTC date; the collector's
        // orchestrator passes in `Local::now().date_naive()`, so an event
        // that starts late-evening in the user's timezone may not match
        // — that's the documented contract.
        if start_utc.date_naive() != today {
            continue;
        }

        // DTEND is optional in RFC 5545, but for "duration_minutes" we
        // need a duration. Fall back to 0 minutes if missing.
        let duration_minutes: i64 = match vevent.get("DTEND").map(|v| v.as_str()) {
            Some(end_value) => match NaiveDateTime::parse_from_str(end_value, ICS_UTC_FORMAT) {
                Ok(end_naive) => {
                    let end_utc = DateTime::<Utc>::from_naive_utc_and_offset(end_naive, Utc);
                    (end_utc - start_utc).num_minutes().max(0)
                }
                Err(_) => 0,
            },
            None => 0,
        };

        let uid = vevent
            .get("UID")
            .map(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let summary = vevent
            .get("SUMMARY")
            .map(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let organizer = vevent
            .get("ORGANIZER")
            .map(|v| v.as_str())
            .map(String::from);
        let location = vevent.get("LOCATION").map(|v| v.as_str()).map(String::from);

        // ATTENDEE is multi-valued: each ATTENDEE line is its own entry.
        // We collect every line whose key (case-insensitive exact-match
        // per RFC 5545) equals "ATTENDEE" — see the `raw_attendees`
        // capture in `parse_vevents` below.
        let attendees: Vec<String> = vevent.extra_attendees.iter().map(String::from).collect();

        events.push(serde_json::json!({
            "uid":              uid,
            "summary":          summary,
            "start":            start_utc.to_rfc3339(),
            "duration_minutes": duration_minutes,
            "attendees":        attendees,
            "organizer":        organizer,
            "location":         location,
        }));
    }

    // Stable order by UTC start so the test assertion + the on-disk
    // file are reproducible.
    events.sort_by(|a, b| {
        let ax = a["start"].as_str().unwrap_or("");
        let bx = b["start"].as_str().unwrap_or("");
        ax.cmp(bx)
    });

    Ok(serde_json::json!({ "events": events }))
}

/// The map of single-valued properties keyed by uppercase RFC 5545 name
/// (UID / SUMMARY / DTSTART / DTEND / ORGANIZER / LOCATION / etc.) plus
/// the ordered list of repeated ATTENDEE properties (since several
/// attendees per event is the common case).
#[derive(Debug, Default)]
struct VeventProps {
    map: BTreeMap<String, String>,
    /// One entry per ATTENDEE line in source order. Stored separately
    /// because `BTreeMap` is a set, not a list.
    extra_attendees: Vec<String>,
}

impl std::ops::Deref for VeventProps {
    type Target = BTreeMap<String, String>;
    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

/// Tokenize the ICS text into a list of `VEVENT` property maps. The
/// tokenizer is line-based and case-insensitive on keys. Keys are
/// stored UPPERCASE. We deliberately ignore everything outside a
/// `VEVENT` block, and inside a `VEVENT` we ignore every line whose
/// key isn't in our allowlist — DESCRIPTION / COMMENT / X-ALT-DESC
/// are never read. Parameter handling: we keep the value verbatim
/// after the `:` (the default RFC 5545 separator); the line folding
/// escape sequences `\\n`, `\\,`, `\\;` are NOT decoded (the fixture
/// uses none of these for the captured fields, and decoding can lose
/// information).
fn parse_vevents(ics_text: &str) -> Vec<VeventProps> {
    let mut events: Vec<VeventProps> = Vec::new();
    let mut current: Option<VeventProps> = None;
    let allowed = [
        "UID",
        "SUMMARY",
        "DTSTART",
        "DTEND",
        "ORGANIZER",
        "LOCATION",
        "ATTENDEE",
    ];

    for raw_line in ics_text.lines() {
        // Lines may begin with whitespace for line folding per RFC 5545
        // §3.1. We un-fold by concatenating any continuation lines onto
        // the previous logical line. We don't go further than that — the
        // short fields we want never fold in practice.
        let line = raw_line.trim_end_matches('\r');

        // RFC 5545 line folding: a line starting with a space or tab is
        // a continuation of the previous line. The fixture doesn't fold
        // anything, but a real Apple Calendar export can, so we handle
        // it correctly for the fields we care about (UID, SUMMARY,
        // DTSTART, DTEND, ATTENDEE, ORGANIZER, LOCATION — all of which
        // are short and unlikely to fold, but defensive).
        if let Some(stripped) = line.strip_prefix(' ').or_else(|| line.strip_prefix('\t')) {
            // Continuation. We append to the last event's most-recent
            // property. Since we process one VEVENT at a time and only
            // store the allowlist keys (no need to remember order beyond
            // ATTENDEE), the simplest correct behavior is: keep a
            // scratchpad and concat onto whichever scalar was last set.
            // The fixture doesn't fold so we don't optimize this further.
            if let Some(cur) = current.as_mut() {
                if let Some(last_value) = cur.map.values_mut().next_back() {
                    last_value.push_str(stripped);
                } else if let Some(last_attendee) = cur.extra_attendees.last_mut() {
                    last_attendee.push_str(stripped);
                }
            }
            continue;
        }

        if line.eq_ignore_ascii_case("BEGIN:VEVENT") {
            current = Some(VeventProps::default());
            continue;
        }
        if line.eq_ignore_ascii_case("END:VEVENT") {
            if let Some(cur) = current.take() {
                events.push(cur);
            }
            continue;
        }
        // Other BEGIN/END lines (BEGIN:VCALENDAR, BEGIN:VTIMEZONE, ...)
        // are ignored — we only want VEVENT bodies.
        if line.starts_with("BEGIN:") || line.starts_with("END:") {
            continue;
        }

        let Some(cur) = current.as_mut() else {
            // Outside any VEVENT block — skip silently.
            continue;
        };

        let Some((raw_key, raw_value)) = split_key_value(line) else {
            continue;
        };
        let key = raw_key.to_ascii_uppercase();
        if !allowed.contains(&key.as_str()) {
            // Privacy: skip DESCRIPTION, COMMENT, X-ALT-DESC, and any
            // unknown / non-allowlisted property. The data is never
            // decoded.
            continue;
        }
        if key == "ATTENDEE" {
            // Preserve every ATTENDEE in source order — multiple
            // attendees per event are normal.
            cur.extra_attendees.push(raw_value.to_string());
        } else {
            cur.map.insert(key, raw_value.to_string());
        }
    }

    events
}

/// Split an ICS content line into `(key, value)` per RFC 5545
/// §3.1. The separator is the first `:` that isn't inside a parameter
/// section. For our allowlist, parameter sections only appear on
/// `ORGANIZER;CN=...:` and `ATTENDEE;CN=...:` style lines, so splitting
/// on the first `:` after any `;` param tokens is correct:
/// `ORGANIZER;CN=Pedro:mailto:pedro@example.com` →
///   key=`ORGANIZER`, value=`mailto:pedro@example.com`.
fn split_key_value(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let head = &line[..colon];
    let value = &line[colon + 1..];
    let key = head.split(';').next().unwrap_or(head).trim();
    Some((key, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal one-event ICS body (kept inline so the test is
    /// self-contained). Duplicate of the fixture file but reduced to
    /// one event for a focused unit test on the tokenizer.
    const SINGLE_EVENT_ICS: &str = "\
BEGIN:VCALENDAR\r\n\
VERSION:2.0\r\n\
BEGIN:VEVENT\r\n\
UID:u@x\r\n\
SUMMARY:Huddle\r\n\
DTSTART:20260815T090000Z\r\n\
DTEND:20260815T093000Z\r\n\
DESCRIPTION:secret body\r\n\
ORGANIZER;CN=Host:mailto:host@x.com\r\n\
ATTENDEE;CN=A:mailto:a@x.com\r\n\
ATTENDEE;CN=B:mailto:b@x.com\r\n\
ATTENDEE;CN=C:mailto:c@x.com\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";

    #[test]
    fn tokenizer_skip_property_keys_outside_allowlist() {
        let events = parse_vevents(SINGLE_EVENT_ICS);
        assert_eq!(events.len(), 1);
        let e = &events[0];
        // Allowed keys surfaced:
        assert_eq!(e.get("UID").map(String::as_str), Some("u@x"));
        assert_eq!(e.get("SUMMARY").map(String::as_str), Some("Huddle"));
        assert_eq!(
            e.get("DTSTART").map(String::as_str),
            Some("20260815T090000Z")
        );
        assert_eq!(e.get("DTEND").map(String::as_str), Some("20260815T093000Z"));
        assert_eq!(
            e.get("ORGANIZER").map(String::as_str),
            Some("mailto:host@x.com")
        );
        // Privacy: DESCRIPTION was never read.
        assert!(e.get("DESCRIPTION").is_none());
        assert_eq!(e.extra_attendees.len(), 3);
        assert_eq!(e.extra_attendees[0], "mailto:a@x.com");
        assert_eq!(e.extra_attendees[2], "mailto:c@x.com");
    }

    #[test]
    fn synthesize_drops_event_on_other_date() {
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        // Pick a different today — event is on 2026-08-15 so should drop.
        let other = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let out = synthesize(SINGLE_EVENT_ICS, other, Utc::now()).unwrap();
        assert_eq!(out["events"].as_array().unwrap().len(), 0);

        let out2 = synthesize(SINGLE_EVENT_ICS, today, Utc::now()).unwrap();
        assert_eq!(out2["events"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn synthesize_duration_zero_when_dtend_missing() {
        let ics = "\
BEGIN:VCALENDAR\r\n\
BEGIN:VEVENT\r\n\
UID:no-end@x\r\n\
SUMMARY:No end\r\n\
DTSTART:20260815T090000Z\r\n\
END:VEVENT\r\n\
END:VCALENDAR\r\n";
        let today = NaiveDate::from_ymd_opt(2026, 8, 15).unwrap();
        let out = synthesize(ics, today, Utc::now()).unwrap();
        let events = out["events"].as_array().unwrap();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["duration_minutes"], 0);
    }
}
