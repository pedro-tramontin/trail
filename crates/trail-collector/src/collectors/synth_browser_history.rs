//! Pure-function transformer: `Vec<RawHistoryRow>` →
//! `TrailRawBrowserHistory` payload.
//!
//! The `browser_history/{chromium,firefox,safari}.rs` readers own the
//! I/O (copying the locked browser SQLite DB to a temp file,
//! querying it for today's visits, returning `RawHistoryRow`
//! structs); this module is the pure transform so the synthesis
//! step is fully testable without any on-disk fixtures. Decoupling
//! also keeps the supervisor (`collect.rs`) honest: validation
//! runs against the transformed output, never raw `History` bytes.
//!
//! **Privacy posture (plan
//! `.hermes/plans/2026-08-11_browser-history-collector.md` §D1):**
//! the synthesizer captures every field the schema declares —
//! `url`, `title`, `last_visit_time`, `visit_count`, `browser`,
//! `profile`, plus the Loose-tier fields `transition_type`,
//! `typed_count`, and `referrer_url`. PII scrubbing is the
//! downstream `src-tauri/src/anonymizer.rs::anonymize` pass's job,
//! running on the laptop before the payload reaches the VPS. This
//! matches the post-PR #219 capture-then-anonymize posture the
//! user confirmed for the calendar collector (same plan §D5).
//!
//! **Time window:** the supervisor passes `today` as the local
//! date the user is reviewing. `RawHistoryRow.last_visit_time` is
//! converted from the browser's native timestamp (Chromium uses
//! WebKit/1601 epoch µs, Firefox uses Unix seconds, Safari uses
//! Cocoa/2001 epoch seconds — see the per-reader modules) into UTC
//! at read time. Rows whose UTC date isn't `today` are dropped.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::{json, Value};

/// Browser kind — drives the reader selection and appears in each
/// payload row so the summarizer can attribute URLs to the
/// browser the user was using.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Browser {
    Chrome,
    Brave,
    Opera,
    Firefox,
    Safari,
}

impl Browser {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Brave => "brave",
            Self::Opera => "opera",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
        }
    }
}

/// One raw row from a browser DB read. The reader is responsible
/// for converting the browser-native timestamp to UTC before
/// passing the row in. `transition_type` is the normalized
/// `core::enums::TransitionType` string ("link", "typed", etc.);
/// readers map the browser-native enum into this canonical set.
/// `typed_count` is `Some(_)` for Chromium only; Firefox/Safari
/// readers pass `None`. `referrer_url` is `Some(_)` if the row's
/// `from_visit` resolved to a real URL — direct navigations are
/// `None`.
#[derive(Debug, Clone)]
pub struct RawHistoryRow {
    pub url: String,
    pub title: String,
    pub last_visit_time: DateTime<Utc>,
    pub visit_count: u32,
    pub browser: Browser,
    pub profile: String,
    pub transition_type: Option<String>,
    pub typed_count: Option<u32>,
    pub referrer_url: Option<String>,
}

/// Build the raw `payload` object for the browser-history source
/// from a pre-loaded list of rows. Pure: same inputs ⇒ same
/// output.
///
/// `rows` is the flat list of `RawHistoryRow`s assembled by the
/// reader pass. `today` is the local date the user is reviewing;
/// rows whose UTC date isn't `today` are dropped. `_now` is
/// reserved for future "as-of" overrides (e.g. backfilling).
pub fn synthesize(rows: &[RawHistoryRow], today: NaiveDate, _now: DateTime<Utc>) -> Result<Value> {
    let mut entries: Vec<Value> = Vec::new();

    for row in rows {
        if row.last_visit_time.date_naive() != today {
            continue;
        }
        entries.push(json!({
            "url":             row.url,
            "title":           row.title,
            "last_visit_time": row.last_visit_time.to_rfc3339(),
            "visit_count":     row.visit_count,
            "browser":         row.browser.as_str(),
            "profile":         row.profile,
            "transition_type": row.transition_type,
            "typed_count":     row.typed_count,
            "referrer_url":    row.referrer_url,
        }));
    }

    // Stable order: most-recent first. The summarizer reads top-down
    // so the user's actual browsing pattern is most-visible.
    entries.sort_by(|a, b| {
        let ax = a["last_visit_time"].as_str().unwrap_or("");
        let bx = b["last_visit_time"].as_str().unwrap_or("");
        bx.cmp(ax)
    });

    Ok(json!({ "entries": entries }))
}

/// Wrap the synthesized payload in the supervisor-expected
/// `RawOutput` envelope (`source | captured_at | date | payload`).
/// Used by every reader's `run()` function.
pub fn envelope(
    payload: Value,
    today: NaiveDate,
    captured_at: DateTime<Utc>,
) -> Result<super::RawOutput> {
    Ok(super::RawOutput {
        source: "browser_history".to_string(),
        captured_at,
        date: today,
        payload,
    })
}

/// Convenience helper for empty results — emits a valid empty
/// envelope (`entries: []`) when the user's pick list intersects
/// with no available browsers.
pub fn empty_envelope(today: NaiveDate, captured_at: DateTime<Utc>) -> Result<super::RawOutput> {
    envelope(
        json!({ "entries": [] }),
        today,
        captured_at,
    )
    .context("building empty browser_history envelope")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 8, 11).unwrap()
    }

    fn row_at(
        day_offset_days: i64,
        url: &str,
        browser: Browser,
        transition: Option<&str>,
        typed: Option<u32>,
        referrer: Option<&str>,
    ) -> RawHistoryRow {
        let ts = Utc.with_ymd_and_hms(2026, 8, 11, 12, 0, 0).unwrap()
            + chrono::Duration::days(day_offset_days);
        RawHistoryRow {
            url: url.to_string(),
            title: format!("title for {url}"),
            last_visit_time: ts,
            visit_count: 1,
            browser,
            profile: "Default".to_string(),
            transition_type: transition.map(String::from),
            typed_count: typed,
            referrer_url: referrer.map(String::from),
        }
    }

    #[test]
    fn synthesize_filters_to_today_only() {
        let rows = vec![
            row_at(0, "https://a/", Browser::Chrome, Some("link"), Some(0), None),
            row_at(-1, "https://yesterday/", Browser::Chrome, Some("link"), Some(0), None),
            row_at(1, "https://tomorrow/", Browser::Chrome, Some("link"), Some(0), None),
        ];
        let out = synthesize(&rows, today(), Utc::now()).unwrap();
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["url"], "https://a/");
    }

    #[test]
    fn synthesize_orders_most_recent_first() {
        let rows = vec![
            row_at(0, "https://first/", Browser::Chrome, Some("link"), Some(0), None),
            // The row_at helper uses the same base timestamp + offset;
            // give the second row a later timestamp by ordering the
            // input list deliberately.
            RawHistoryRow {
                url: "https://later/".to_string(),
                title: "later".to_string(),
                last_visit_time: Utc.with_ymd_and_hms(2026, 8, 11, 18, 0, 0).unwrap(),
                visit_count: 1,
                browser: Browser::Chrome,
                profile: "Default".to_string(),
                transition_type: Some("link".to_string()),
                typed_count: Some(0),
                referrer_url: None,
            },
        ];
        let out = synthesize(&rows, today(), Utc::now()).unwrap();
        let entries = out["entries"].as_array().unwrap();
        assert_eq!(entries[0]["url"], "https://later/");
        assert_eq!(entries[1]["url"], "https://first/");
    }

    #[test]
    fn synthesize_emits_all_loose_tier_fields() {
        let rows = vec![RawHistoryRow {
            url: "https://example.com/search?q=foo".to_string(),
            title: "Search result".to_string(),
            last_visit_time: Utc.with_ymd_and_hms(2026, 8, 11, 9, 30, 0).unwrap(),
            visit_count: 3,
            browser: Browser::Chrome,
            profile: "Profile 1".to_string(),
            transition_type: Some("typed".to_string()),
            typed_count: Some(1),
            referrer_url: Some("https://google.com".to_string()),
        }];
        let out = synthesize(&rows, today(), Utc::now()).unwrap();
        let e = &out["entries"][0];
        assert_eq!(e["transition_type"], "typed");
        assert_eq!(e["typed_count"], 1);
        assert_eq!(e["referrer_url"], "https://google.com");
        assert_eq!(e["browser"], "chrome");
        assert_eq!(e["profile"], "Profile 1");
    }

    #[test]
    fn synthesize_drops_typed_count_for_firefox() {
        let rows = vec![RawHistoryRow {
            url: "https://example.com".to_string(),
            title: "x".to_string(),
            last_visit_time: Utc.with_ymd_and_hms(2026, 8, 11, 10, 0, 0).unwrap(),
            visit_count: 1,
            browser: Browser::Firefox,
            profile: "default-release".to_string(),
            transition_type: Some("link".to_string()),
            typed_count: None, // Firefox doesn't track this
            referrer_url: None,
        }];
        let out = synthesize(&rows, today(), Utc::now()).unwrap();
        assert!(out["entries"][0]["typed_count"].is_null());
    }

    #[test]
    fn empty_envelope_has_empty_entries_array() {
        let env = empty_envelope(today(), Utc::now()).unwrap();
        assert_eq!(env.source, "browser_history");
        assert_eq!(env.date, today());
        assert_eq!(env.payload["entries"].as_array().unwrap().len(), 0);
    }
}