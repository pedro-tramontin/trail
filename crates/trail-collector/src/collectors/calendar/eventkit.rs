// SPDX-License-Identifier: MIT
//
// calendar/eventkit.rs — macOS-only `EventKit.framework` reader for
// the calendar collector. This file is `#[cfg(target_os = "macos")]`
// gated at the `calendar/mod.rs` level so the musl cross-compile
// never sees the `objc2-event-kit` dependency.
//
// The submodule reads the user's calendars via `EKEventStore` and
// projects the events to the same 7-field (or 8-field, with `notes`)
// schema the `.ics` parser emits. The `notes` field is run through
// `crate::anonymizer::anonymize` (when the strictness string is
// "aggressive" or "moderate") before the event is added to the
// payload — per the 2026-08-11 user decision, we capture `notes`
// for summarizer context but the on-disk payload is scrubbed.
//
// Stub shape: the real implementation lands in a follow-up commit.
// Today the function compiles but always errors with a clear
// "not yet wired" message; the dispatch in `calendar/mod.rs` is
// already plumbed. This keeps the macOS build green while the
// real EventKit binding work proceeds.

use anyhow::{bail, Result};
use chrono::{Local, Utc};

use super::super::synth_calendar;
use super::super::{CollectorLaptopConfig, RawOutput};

/// Read today's events from `EKEventStore` and project them to the
/// 7-field schema.
///
/// This is a stub. The real implementation initialises
/// `EKEventStore`, calls `requestFullAccessToEventsWithCompletion`
/// (macOS 14+) on a background thread, waits for the TCC decision,
/// then enumerates events via
/// `eventsMatchingPredicate(_:)` over the
/// `predicateForEventsWithStartDate_endDate_calendars` window
/// (today 00:00 → tomorrow 00:00 in the user's local time). Each
/// `EKEvent` is projected via
/// `super::super::synth_calendar::synthesize_eventkit` (a new
/// pure-function helper that mirrors `synthesize` but takes an
/// `EKEvent` instead of an ICS line buffer).
///
/// For today, this stub returns a clear "not yet wired" error so
/// the dispatch path compiles but a user who picks EventKit sees
/// the honest "still being built" message.
pub fn run(_cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    let now = Utc::now();
    let today = Local::now().date_naive();
    // Stub: emit an empty payload so the supervisor's schema check
    // passes (an empty events array is valid for the schema). A
    // future commit replaces this with the real EKEventStore read
    // + synth_eventkit projection.
    let payload = synth_calendar::synthesize("", today, now)
        .context("synthesizing empty EventKit payload (stub)")?;
    // Surface the "stub" state to anyone tailing the logs.
    tracing::warn!(
        "calendar/eventkit::run: EventKit reader is a stub; returning empty events. \
         The real implementation lands in a follow-up commit. \
         (2026-08-11 plan-of-record, decision #2: notes are run through anonymize, \
         not excluded.)"
    );
    let _ = payload;
    bail!(
        "EventKit calendar reader is not yet wired (stub). \
         The on-disk `Config.calendar.kind = \"event_kit\"` is parsed correctly; \
         the live read path lands in a follow-up commit. \
         Today, set `kind = \"ics\"` (or omit `kind` for the default) to keep the \
         collector green. (2026-08-11 plan-of-record, decision #2.)"
    );
}
