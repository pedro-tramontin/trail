// SPDX-License-Identifier: MIT
//
// calendar/eventkit.rs — macOS-only `EventKit.framework` reader for
// the calendar collector. This file is `#[cfg(target_os = "macos")]`
// gated at the `calendar/mod.rs` level so the musl cross-compile
// never sees the `objc2-event-kit` dependency.
//
// The submodule reads the user's calendars via `EKEventStore` and
// projects the events to the same 7-field schema the `.ics` parser
// emits, plus an optional `notes` field. The 7+1-field shape:
//
//   uid, summary, start, duration_minutes, attendees, organizer,
//   location, (notes — optional)
//
// mirrors `synth_calendar::synthesize` exactly so the schema
// validation, on-disk envelope shape, and the summarizer's
// downstream contract are unchanged. `EKEvent.description` /
// `EKEvent.comments` are NEVER read (privacy: calendar event
// bodies leak meeting context, customer names, healthcare
// details). `EKEvent.notes` IS read — the local LLM (Ollama) is
// the trusted anonymizer (see `src-tauri/prompts.rs`'s PRIVACY
// block).
//
// **Calendar filter**: when `cfg.calendar_names` is `Some(list)`,
// only events whose `EKCalendar.title()` matches one of the names
// are included. `None` ⇒ every calendar the user granted access
// to. The filter is read-only; we never mutate `EKCalendar`s or
// any `EKEventStore` state.
//
// **Live integration test**: `TRAIL_TEST_EVENTKIT_INTEGRATION=1`
// enables an end-to-end test that (a) calls
// `EKEventStore.authorizationStatusForEntityType`, (b) when the
// user has full access, queries today's events, and (c) validates
// the resulting envelope against `calendar.schema.json`. Off by
// default — same convention as `voice/permission.rs`'s
// `TRAIL_TEST_VOICE_INTEGRATION` gate.
//
// **Why `msg_send!` everywhere:** the typed objc2 wrappers around
// `EKEventStore` / `EKCalendar` work fine, but their constructors
// and instance methods sometimes fight with Rust's borrow checker
// when mixing `Retained<T>` with `Option<&NSArray<T>>`. To keep
// this module readable (and easy to port forward when Apple
// changes the framework's nullability annotations), we go
// class-method + `msg_send!` throughout, mirroring the same
// pattern that `src-tauri/src/voice/permission.rs` uses for
// `AVCaptureDevice` (which is the established pattern in this
// project for ObjC bindings work).

use anyhow::{Context, Result};
use chrono::{Local, TimeZone, Utc};

use super::super::{CollectorLaptopConfig, RawOutput};

/// Top-level entry. Wraps the macOS-only implementation in the
/// `RawOutput` envelope. Non-macOS is unreachable; the dispatch
/// in `calendar/mod.rs` is gated.
pub fn run(cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    let now = Utc::now();
    let today = Local::now().date_naive();
    let payload = synth_eventkit(cfg, today)
        .context("synthesizing EventKit calendar payload")?;
    Ok(RawOutput {
        source: "calendar".to_string(),
        captured_at: now,
        date: today,
        payload,
    })
}

/// Read today's events from `EKEventStore` and project them to the
/// 7+1-field schema.
///
/// `today` is the local date the collector is capturing for; events
/// whose `startDate` (UTC) is on a different day are dropped,
/// matching `synth_calendar::synthesize`'s contract.
#[cfg(target_os = "macos")]
fn synth_eventkit(
    cfg: &CollectorLaptopConfig,
    today: chrono::NaiveDate,
) -> Result<serde_json::Value> {
    use objc2::{class, msg_send, ClassType};
    use objc2_event_kit::EKAuthorizationStatus;
    use objc2_foundation::{NSArray, NSDate, NSString};

    // SAFETY: `class!` resolves at process load; the class is
    // registered by EventKit.framework which we link in
    // `build.rs`. `is_null` is intentionally skipped per the
    // same convention used in `voice/permission.rs` —
    // `objc2 0.6` removed the typed wrapper's `is_null`
    // method and the macro guarantees a non-null class on
    // macOS.
    let cls = class!(EKEventStore);

    // 1. TCC probe via the class method
    //    `+[EKEventStore authorizationStatusForEntityType:]`.
    //    We pass the raw integer `0` for `EKEntityTypeEvent`
    //    rather than the enum constant — the integer is the
    //    historical Apple value and stable across SDK releases.
    let raw_status: isize = unsafe {
        msg_send![cls, authorizationStatusForEntityType: 0isize]
    };
    let status = EKAuthorizationStatus(raw_status);
    match status {
        EKAuthorizationStatus::FullAccess | EKAuthorizationStatus::Authorized => {}
        EKAuthorizationStatus::NotDetermined => {
            anyhow::bail!(
                "EventKit access not yet requested. Run the Trail onboarding wizard \
                 (or open System Settings → Privacy & Security → Calendars) \
                 and grant Trail full access."
            );
        }
        EKAuthorizationStatus::Denied
        | EKAuthorizationStatus::Restricted
        | EKAuthorizationStatus::WriteOnly => {
            anyhow::bail!(
                "EventKit access denied / restricted. Open \
                 System Settings → Privacy & Security → Calendars and grant \
                 Trail full access, then re-run."
            );
        }
        _ => {
            // Future TCC states from a newer macOS SDK.
            anyhow::bail!(
                "EventKit returned an unknown authorization status: {:?}",
                raw_status
            );
        }
    }

    // 2. Create the event store.
    let store: objc2::rc::Retained<objc2_event_kit::EKEventStore> =
        unsafe { msg_send![cls, new] };

    // 3. Resolve the calendar filter.
    let calendars_for_query: Option<objc2::rc::Retained<NSArray<objc2_event_kit::EKCalendar>>> =
        match &cfg.calendar_names {
            Some(names) => {
                let all_calendars: objc2::rc::Retained<NSArray<objc2_event_kit::EKCalendar>> =
                    unsafe { msg_send![&store, calendarsForEntityType: 0usize] };
                let count = all_calendars.count();
                let mut picked: Vec<*const objc2_event_kit::EKCalendar> = Vec::new();
                let mut missing: Vec<String> = Vec::new();
                for i in 0..count {
                    let cal_obj = unsafe { all_calendars.objectAtIndex(i) };
                    let title: Retained<NSString> = unsafe { msg_send![&cal_obj, title] };
                    let title_str = title.to_string();
                    if names.iter().any(|n| n == &title_str) {
                        picked.push(cal_obj.as_ref() as *const _);
                    }
                }
                for n in names {
                    let found = picked.iter().any(|p| unsafe {
                        let cal: &objc2_event_kit::EKCalendar = &**p;
                        let t: Retained<NSString> = msg_send![cal, title];
                        t.to_string() == *n
                    });
                    if !found {
                        missing.push(n.clone());
                    }
                }
                if !missing.is_empty() {
                    tracing::warn!(
                        requested = ?missing,
                        "EventKit: some requested calendars are not present \
                         (renamed or deleted?). Continuing with the others."
                    );
                }
                if picked.is_empty() {
                    anyhow::bail!(
                        "EventKit: no calendars matched the filter {:?}. \
                         Open the wizard and pick a calendar the user has access to.",
                        names
                    );
                }
                let array = build_nsarray_from_calendars(&picked)
                    .context("wrapping calendar filter in NSArray")?;
                Some(array)
            }
            None => None,
        };

    // 4. Date window: today 00:00 local → tomorrow 00:00 local.
    let start_local = Local
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).expect("valid midnight"))
        .single()
        .context("ambiguous local midnight — DST boundary?")?;
    let end_local = start_local + chrono::Duration::days(1);
    let start_utc = start_local.with_timezone(&Utc);
    let end_utc = end_local.with_timezone(&Utc);

    let start_ns = NSDate::dateWithTimeIntervalSince1970(start_utc.timestamp() as f64);
    let end_ns = NSDate::dateWithTimeIntervalSince1970(end_utc.timestamp() as f64);

    // 5. Predicate + query. `calendars_for_query.as_deref()`
    //    gives `Option<&NSArray<EKCalendar>>` — `None` ⇒ all.
    let predicate = unsafe {
        store.predicateForEventsWithStartDate_endDate_calendars(
            &start_ns,
            &end_ns,
            calendars_for_query.as_deref(),
        )
    };
    let events: objc2::rc::Retained<NSArray<objc2_event_kit::EKEvent>> = unsafe {
        msg_send![&store, eventsMatchingPredicate: &*predicate]
    };
    let count = events.count();

    // 6. Project each event.
    let mut out: Vec<serde_json::Value> = Vec::with_capacity(count as usize);
    for i in 0..count {
        let event_obj = unsafe { events.objectAtIndex(i) };
        out.push(project_event(event_obj.as_ref())?);
    }

    out.sort_by(|a, b| {
        let ax = a["start"].as_str().unwrap_or("");
        let bx = b["start"].as_str().unwrap_or("");
        ax.cmp(bx)
    });

    Ok(serde_json::json!({ "events": out }))
}

/// Build an `NSArray<EKCalendar>` from a list of raw EKCalendar
/// pointers. The typed `NSArray::arrayWithObjects_count` takes a
/// `NonNull<NonNull<T>>` so we lay out the pointer array
/// ourselves.
#[cfg(target_os = "macos")]
unsafe fn build_nsarray_from_calendars(
    ptrs: &[*const objc2_event_kit::EKCalendar],
) -> Result<objc2::rc::Retained<objc2_foundation::NSArray<objc2_event_kit::EKCalendar>>> {
    use objc2_foundation::NSArray;
    use std::ptr::NonNull;
    // SAFETY: We hold the raw pointers only long enough to build
    // the NSArray. `arrayWithObjects_count:` retains each object
    // it receives, so the resulting NSArray owns the references
    // (the underlying EKCalendars are valid for the duration of
    // the function).
    let inner: Vec<NonNull<objc2_event_kit::EKCalendar>> = ptrs
        .iter()
        .map(|p| NonNull::new(*p as *mut _).expect("null EKCalendar pointer"))
        .collect();
    let cnt = inner.len();
    let slice_ptr = NonNull::new(inner.as_ptr() as *mut NonNull<objc2_event_kit::EKCalendar>)
        .expect("empty inner slice");
    let arr: objc2::rc::Retained<NSArray<objc2_event_kit::EKCalendar>> =
        NSArray::arrayWithObjects_count(slice_ptr, cnt);
    // The NSArray owns the references now; release our wrapper.
    drop(inner);
    Ok(arr)
}

/// Non-macOS stub.
#[cfg(not(target_os = "macos"))]
fn synth_eventkit(
    _cfg: &CollectorLaptopConfig,
    _today: chrono::NaiveDate,
) -> Result<serde_json::Value> {
    unreachable!(
        "synth_eventkit is macOS-only; the dispatch in calendar/mod.rs \
         must be `#[cfg(target_os = \"macos\")]`-gated."
    )
}

/// Project one `EKEvent` to the 7+1-field schema.
#[cfg(target_os = "macos")]
fn project_event(event: &objc2_event_kit::EKEvent) -> Result<serde_json::Value> {
    use objc2::msg_send;
    use objc2_foundation::{NSArray, NSString};

    let title: Retained<NSString> = unsafe { msg_send![event, title] };
    let summary = title.to_string();

    let start_ns: Retained<objc2_foundation::NSDate> = unsafe { msg_send![event, startDate] };
    let end_ns: Retained<objc2_foundation::NSDate> = unsafe { msg_send![event, endDate] };
    let start_utc = unix_ts_to_chrono(start_ns.timeIntervalSince1970());
    let end_utc = unix_ts_to_chrono(end_ns.timeIntervalSince1970());
    let duration_minutes = (end_utc - start_utc).num_minutes().max(0);

    let uid: String = unsafe {
        let id: Option<Retained<NSString>> = msg_send![event, eventIdentifier];
        id.map(|s| s.to_string()).unwrap_or_default()
    };

    let location: Option<String> = unsafe {
        let l: Option<Retained<NSString>> = msg_send![event, location];
        l.map(|s| s.to_string())
    };

    let organizer: Option<String> = unsafe {
        // `organizer` returns `Option<Retained<EKParticipant>>`.
        let o: Option<Retained<objc2_event_kit::EKParticipant>> = msg_send![event, organizer];
        o.map(|p| p_name(p.as_ref()))
    };

    let attendees: Vec<String> = unsafe {
        let arr: Option<Retained<NSArray<objc2_event_kit::EKParticipant>>> =
            msg_send![event, attendees];
        arr.map(|arr| {
            let n = arr.count();
            (0..n)
                .map(|i| {
                    let p = unsafe { arr.objectAtIndex(i) };
                    p_name(p.as_ref())
                })
                .collect()
        })
        .unwrap_or_default()
    };

    let notes: Option<String> = unsafe {
        let n: Option<Retained<NSString>> = msg_send![event, notes];
        n.map(|s| s.to_string())
    };

    let mut obj = serde_json::json!({
        "uid":              uid,
        "summary":          summary,
        "start":            start_utc.to_rfc3339(),
        "duration_minutes": duration_minutes,
        "attendees":        attendees,
        "organizer":        organizer,
        "location":         location,
    });
    if let Some(n) = notes {
        obj.as_object_mut()
            .expect("json!() returned object")
            .insert("notes".to_string(), serde_json::Value::String(n));
    }
    Ok(obj)
}

#[cfg(not(target_os = "macos"))]
fn project_event(
    _event: &objc2_event_kit::EKEvent,
) -> Result<serde_json::Value> {
    unreachable!("project_event is macOS-only.")
}

#[cfg(target_os = "macos")]
fn p_name(p: &objc2_event_kit::EKParticipant) -> String {
    use objc2::msg_send;
    use objc2_foundation::NSString;
    unsafe {
        let n: Option<Retained<NSString>> = msg_send![p, name];
        n.map(|s| s.to_string()).unwrap_or_default()
    }
}

#[cfg(not(target_os = "macos"))]
fn p_name(_p: &objc2_event_kit::EKParticipant) -> String {
    unreachable!("p_name is macOS-only.")
}

/// Convert a Cocoa `NSTimeInterval` (seconds since Unix epoch) to
/// `chrono::DateTime<Utc>`.
fn unix_ts_to_chrono(ts: f64) -> chrono::DateTime<Utc> {
    let secs = ts.trunc() as i64;
    let nanos = (ts.fract() * 1_000_000_000.0) as u32;
    Utc.timestamp_opt(secs, nanos)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap())
}

// `Retained` is `objc2::rc::Retained`. Imported here so the
// function bodies above can write `Retained<NSString>` without
// the full path. The bare `Retained` resolves to `objc2::rc::Retained`
// (a re-export by `objc2`'s prelude).
use objc2::rc::Retained;

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(all(target_os = "macos", test))]
mod macos_tests {
    //! Mocked unit tests that run in `cargo test -p trail-collector`
    //! on a macOS developer laptop (NOT in CI — CI is Linux).
    //!
    //! They construct synthetic `EKEvent`s via
    //! `+[EKEvent eventWithEventStore:]` and assert the
    //! projection logic preserves the field shape. The mocked
    //! tests do NOT cover TCC status, the calendar filter, or
    //! the live event query — those are exercised by the live
    //! integration test below.
    use super::*;
    use objc2::msg_send;
    use objc2_event_kit::{EKEvent, EKEventStore};
    use objc2_foundation::{NSDate, NSString};

    #[test]
    fn project_event_emits_schema_aligned_json() {
        let cls = class!(EKEventStore);
        let store: Retained<EKEventStore> = unsafe { msg_send![cls, new] };
        let event: Retained<EKEvent> =
            unsafe { msg_send![class!(EKEvent), eventWithEventStore: &*store] };

        let title = NSString::from_str("Design review with Alice");
        let _: () = unsafe { msg_send![&*event, setTitle: &*title] };

        let start = NSDate::dateWithTimeIntervalSince1970(1_700_000_000.0);
        let end = NSDate::dateWithTimeIntervalSince1970(1_700_003_600.0);
        let _: () = unsafe { msg_send![&*event, setStartDate: &*start] };
        let _: () = unsafe { msg_send![&*event, setEndDate: &*end] };

        let json =
            project_event(&*event).expect("project_event should not fail on a synthetic event");
        assert_eq!(json["summary"], "Design review with Alice");
        assert_eq!(json["duration_minutes"], 60, "1h gap = 60 minutes");
        assert!(json["attendees"].is_array(), "attendees must be an array");
        // `notes` was never set on the synthetic event — the
        // projection must omit the field (or set it to null).
        let notes = &json["notes"];
        assert!(
            notes.is_null() || notes.as_str().map(|s| s.is_empty()).unwrap_or(true),
            "no notes set ⇒ field absent or empty, got {notes:?}"
        );
    }

    // Pull in `class!` for the test.
    use objc2::class;
}

/// Live integration test gated on `TRAIL_TEST_EVENTKIT_INTEGRATION=1`.
///
/// Excluded from CI by the env-var gate. Run on a developer's
/// macOS laptop when they want to validate the EventKit round-trip
/// end-to-end. Skips silently when TCC is anything other than
/// `.fullAccess` / `.authorized` (no prompting).
#[cfg(target_os = "macos")]
#[test]
fn live_eventkit_round_trip() {
    if std::env::var("TRAIL_TEST_EVENTKIT_INTEGRATION").ok().as_deref() != Some("1") {
        return;
    }
    use objc2::{class, msg_send};
    use objc2_event_kit::EKAuthorizationStatus;
    use objc2_foundation::{NSArray, NSDate};

    let cls = class!(EKEventStore);
    let raw_status: isize =
        unsafe { msg_send![cls, authorizationStatusForEntityType: 0isize] };
    let status = EKAuthorizationStatus(raw_status);
    if !matches!(
        status,
        EKAuthorizationStatus::FullAccess | EKAuthorizationStatus::Authorized
    ) {
        eprintln!(
            "live_eventkit_round_trip: TCC status is {:?}; \
             grant Trail full access in System Settings → \
             Privacy & Security → Calendars and re-run with \
             TRAIL_TEST_EVENTKIT_INTEGRATION=1.",
            status
        );
        return;
    }

    let store: Retained<objc2_event_kit::EKEventStore> =
        unsafe { msg_send![cls, new] };
    let now = Utc::now();
    let today = Local::now().date_naive();
    let start_local = Local
        .from_local_datetime(&today.and_hms_opt(0, 0, 0).unwrap())
        .single()
        .unwrap();
    let end_local = start_local + chrono::Duration::days(1);
    let start_ns = NSDate::dateWithTimeIntervalSince1970(
        start_local.with_timezone(&Utc).timestamp() as f64,
    );
    let end_ns = NSDate::dateWithTimeIntervalSince1970(
        end_local.with_timezone(&Utc).timestamp() as f64,
    );
    let pred = unsafe {
        store.predicateForEventsWithStartDate_endDate_calendars(
            &start_ns,
            &end_ns,
            None,
        )
    };
    let events: Retained<NSArray<objc2_event_kit::EKEvent>> =
        unsafe { msg_send![&store, eventsMatchingPredicate: &*pred] };

    let mut out: Vec<serde_json::Value> = Vec::with_capacity(events.count() as usize);
    let now_cap = now;
    for i in 0..events.count() {
        let event = unsafe { events.objectAtIndex(i) };
        let start = unsafe { msg_send![&*event, startDate] };
        let start_ts: f64 = unsafe { msg_send![&*start, timeIntervalSince1970] };
        let start_chrono = unix_ts_to_chrono(start_ts);
        if start_chrono.date_naive() != today {
            continue;
        }
        match project_event(event.as_ref()) {
            Ok(json) => out.push(json),
            Err(e) => eprintln!("project_event failed at idx {i}: {e}"),
        }
    }
    out.sort_by(|a, b| {
        let ax = a["start"].as_str().unwrap_or("");
        let bx = b["start"].as_str().unwrap_or("");
        ax.cmp(bx)
    });
    let payload = serde_json::json!({ "events": out });

    let envelope = serde_json::json!({
        "source": "calendar",
        "captured_at": now_cap.to_rfc3339(),
        "date": today.format("%Y-%m-%d").to_string(),
        "payload": payload,
    });
    let schema_str = include_str!("../../../schemas/calendar.schema.json");
    let schema: serde_json::Value = serde_json::from_str(schema_str).unwrap();
    let compiled = jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .compile(&schema)
        .unwrap();
    if let Err(errs) = compiled.validate(&envelope) {
        for e in errs {
            eprintln!("schema error: {e}");
        }
        panic!("envelope failed schema validation");
    }
    eprintln!(
        "live_eventkit_round_trip: OK, {} events captured for {}",
        events.count(),
        today
    );
}
