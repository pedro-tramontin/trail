// SPDX-License-Identifier: MIT
//
// browser_history/chromium.rs — reads Chrome / Brave / Opera
// `History` SQLite databases. All three browsers share the
// Chromium schema (they're Chromium-derivative), so one reader
// handles all three; the only per-browser difference is the file
// path (resolved by the scanner in
// `src-tauri/src/onboarding/scan.rs::chrome_brave_history_path`)
// and the per-profile layout (Chrome and Brave use a single
// `Default` profile; Opera uses `Default` or `Opera Stable`).
//
// **SQLite-open strategy:** per plan §D2, we **copy the locked DB
// to a temp file** before opening it. Chromium uses WAL mode by
// default and the file is locked while the browser is running.
// `sqlite3_open_v2(...SQLITE_OPEN_READONLY)` would fail on the
// WAL file; copy is the documented Mozilla/Chromium approach and
// costs ~few ms for a 50MB `History` file. Temp file deleted on
// drop.
//
// **Schema (Chrome 100+ / Brave 1.x / Opera 90+):**
//
//   urls(id INTEGER PRIMARY KEY, url LONGVARCHAR, title LONGVARCHAR,
//        visit_count INTEGER, typed_count INTEGER, last_visit_time
//        INTEGER)              -- WebKit/1601-epoch µs
//
//   visits(id INTEGER PRIMARY KEY, url INTEGER, from_visit INTEGER,
//          transition INTEGER, visit_time INTEGER)
//          -- transition is the ChromePageTransition bitfield
//          -- (0xFF mask = core type; 0xFF00 mask = qualifier)
//
//   VisitSource enum:
//     SOURCE_LINK = 0  (link click)
//     SOURCE_TYPED = 1 (user typed URL)
//     SOURCE_AUTO_BOOKMARK = 5
//     SOURCE_AUTO_SUBFRAME = 6
//     SOURCE_MANUAL_SUBFRAME = 7
//     SOURCE_GENERATED = 8  (e.g. auto-generated bookmarks)
//     SOURCE_START_PAGE = 9  (homepage)
//     SOURCE_FORM_SUBMIT = 11
//     SOURCE_RELOAD = 2
//     SOURCE_KEYWORD = 3  (search keyword)
//     SOURCE_KEYWORD_GENERATED = 4
//     SOURCE_REDIRECT = 20
//
// **Timestamps:** Chromium stores `last_visit_time` as µs since
// the WebKit epoch (1601-01-01 UTC). Conversion: divide by 1_000_000
// to get Unix seconds, then add 11_644_473_600 (the Unix ↔ WebKit
// epoch delta).

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use super::super::synth_browser_history::{Browser, RawHistoryRow};

/// Chromium-derivative readers (Chrome, Brave, Opera) all share
/// this code. `db_path` is the absolute `History` SQLite path
/// (resolved upstream by the scanner); `browser` is which vendor
/// the row came from (used in the payload's `browser` field);
/// `profile` is the profile name (e.g. `Default`, `Profile 1`,
/// `Opera Stable`).
///
/// The function is sync; the supervisor wraps it in a
/// `tokio::task::spawn_blocking` if needed.
pub fn read_chromium(
    db_path: &Path,
    browser: Browser,
    profile: &str,
) -> Result<Vec<RawHistoryRow>> {
    let temp = copy_to_temp(db_path)
        .with_context(|| format!("copying chromium DB {} to temp", db_path.display()))?;
    let conn = Connection::open_with_flags(
        temp.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening chromium DB copy at {}", temp.path().display()))?;

    // The query joins `urls` (URL metadata + visit_count) with the
    // user's most-recent visit per URL. We take the MAX(visit_time)
    // because the `urls` table has one row per URL but the user
    // may have visited it many times (multiple rows in `visits`);
    // we want the latest.
    //
    // The day-window filter is applied in `synth_browser_history::
    // synthesize` after we convert timestamps to UTC, so this
    // query intentionally has no date predicate — we read all
    // rows and let the synthesizer prune. This keeps the per-DB
    // query simple (no timezone math in SQL) and the date filter
    // authoritative in one place.
    let mut stmt = conn.prepare(
        r#"
        SELECT u.url, u.title, u.visit_count, u.typed_count,
               v.transition, v.visit_time, v.from_visit
        FROM urls u
        JOIN visits v ON v.id = (
            SELECT id FROM visits WHERE url = u.id
            ORDER BY visit_time DESC LIMIT 1
        )
        "#,
    )?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let url: String = r.get(0)?;
        let title: String = r.get(1)?;
        let visit_count: i64 = r.get(2)?;
        let typed_count: i64 = r.get(3)?;
        let transition: i64 = r.get(4)?;
        let visit_time_micros: i64 = r.get(5)?;
        let from_visit_id: Option<i64> = r.get(6)?;

        let last_visit_time = webkit_epoch_to_utc(visit_time_micros);
        let transition_type = Some(normalize_chromium_transition(transition));
        let typed = if typed_count > 0 { Some(typed_count as u32) } else { Some(0) };

        // Resolve referrer: from_visit is the id of a row in
        // `visits` whose `url` column is the referrer URL. We
        // execute a second query per row — fine for ~500 rows/day,
        // but worth caching if perf ever bites. The lookups are
        // O(1) on the id index.
        let referrer_url = match from_visit_id {
            Some(id) if id > 0 => {
                let mut s2 = conn
                    .prepare("SELECT u.url FROM visits v JOIN urls u ON u.id = v.url WHERE v.id = ?1")?;
                let r2: Option<String> = s2
                    .query_row([id], |row| row.get(0))
                    .ok();
                r2
            }
            _ => None,
        };

        out.push(RawHistoryRow {
            url,
            title,
            last_visit_time,
            visit_count: visit_count.max(0) as u32,
            browser,
            profile: profile.to_string(),
            transition_type,
            typed_count: typed,
            referrer_url,
        });
    }

    // `temp` is dropped at end of scope; the temp file is removed
    // by `tempfile::NamedTempFile`'s Drop impl. The `_conn` and
    // `_stmt` go away too.
    Ok(out)
}

/// Copy the locked browser DB to a temp file the SQLite reader
/// can open. Returns a `NamedTempFile` whose path is valid for the
/// lifetime of the returned handle.
fn copy_to_temp(src: &Path) -> Result<NamedTempFile> {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("browser-history");
    let prefix = format!("trail-{stem}-");
    let temp = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".sqlite")
        .tempfile_in(parent)
        .context("creating temp file for browser DB copy")?;
    fs::copy(src, temp.path())
        .with_context(|| format!("copying {} to temp", src.display()))?;
    Ok(temp)
}

/// Convert a Chromium `last_visit_time` (µs since WebKit epoch
/// 1601-01-01 UTC) into a `DateTime<Utc>`.
fn webkit_epoch_to_utc(micros: i64) -> DateTime<Utc> {
    // WebKit epoch = 1601-01-01 00:00:00 UTC = Unix epoch
    // 11644473600 seconds later.
    const WEBKIT_TO_UNIX_SECS: i64 = 11_644_473_600;
    let secs = micros / 1_000_000;
    let nanos = ((micros % 1_000_000) * 1000) as u32;
    let unix_secs = secs.saturating_sub(WEBKIT_TO_UNIX_SECS);
    Utc.timestamp_opt(unix_secs, nanos)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap())
}

/// Map a Chromium transition bitfield (lower 8 bits = core type)
/// to the normalized enum string used in the payload.
fn normalize_chromium_transition(transition: i64) -> String {
    // Per Chromium source: core types are 0..=10, with 20 for
    // redirect. The qualifier bits (0xFF00) we ignore for now
    // (FORWARD / BACK / etc. — implicit in the visit ordering).
    let core = transition & 0xFF;
    match core {
        0 => "link",
        1 => "typed",
        2 => "reload",       // SOURCE_RELOAD = 2 in VisitSource
        3 => "keyword",      // SOURCE_KEYWORD
        4 => "keyword_generated",
        5 => "auto_bookmark",
        6 => "auto_subframe",
        7 => "manual_subframe",
        8 => "generated",
        9 => "start_page",
        10 => "form_submit",
        20 => "redirect",
        _ => "other",
    }
    .to_string()
}

/// Resolve the user's pick list (Chromium-derivative browsers
/// only) against the scanner's evidence paths and return one
/// `RawHistoryRow` vector covering all of them. Returns an empty
/// `Vec` if the user picked no Chromium browsers (silent — the
/// Firefox/Safari readers handle their own cases).
pub fn read_all_chromium(db_paths: &[(Browser, PathBuf, String)]) -> Result<Vec<RawHistoryRow>> {
    let mut all = Vec::new();
    for (browser, db_path, profile) in db_paths {
        match read_chromium(db_path, *browser, profile) {
            Ok(rows) => all.extend(rows),
            // If one browser's DB is unreadable (e.g. the user has
            // Chrome but the file is corrupted), don't fail the
            // whole collection — log and continue.
            Err(e) => tracing::warn!(
                browser = browser.as_str(),
                profile = %profile,
                path = %db_path.display(),
                error = %e,
                "skipping chromium browser (DB unreadable)"
            ),
        }
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::synth_browser_history::{synthesize, Browser};
    use chrono::NaiveDate;
    use rusqlite::Connection;

    fn build_chromium_db(path: &Path, rows: &[(&str, &str, i64, i64, i64, i64)]) {
        // rows = (url, title, visit_count, typed_count, transition, visit_time_micros)
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE urls (
                id INTEGER PRIMARY KEY,
                url LONGVARCHAR,
                title LONGVARCHAR,
                visit_count INTEGER,
                typed_count INTEGER,
                last_visit_time INTEGER
            );
            CREATE TABLE visits (
                id INTEGER PRIMARY KEY,
                url INTEGER,
                from_visit INTEGER,
                transition INTEGER,
                visit_time INTEGER
            );
            "#,
        )
        .unwrap();
        for (i, (url, title, vc, tc, t, vt)) in rows.iter().enumerate() {
            let id = (i + 1) as i64;
            conn.execute(
                "INSERT INTO urls (id, url, title, visit_count, typed_count, last_visit_time) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, url, title, vc, tc, vt],
            ).unwrap();
            conn.execute(
                "INSERT INTO visits (id, url, from_visit, transition, visit_time) VALUES (?1, ?2, 0, ?3, ?4)",
                rusqlite::params![id, id, t, vt],
            ).unwrap();
        }
    }

    fn webkit_micros(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        // Build the timestamp from chrono (UTC) → Unix seconds,
        // then add WEBKIT_TO_UNIX_SECS and convert to µs.
        use chrono::TimeZone;
        let ts = chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap();
        let unix = ts.timestamp();
        (unix + 11_644_473_600) * 1_000_000
    }

    #[test]
    fn webkit_epoch_conversion_round_trip() {
        let ts = webkit_micros(2026, 8, 11, 12, 0, 0);
        let back = webkit_epoch_to_utc(ts);
        // 2026-08-11 12:00:00 UTC = Unix 1_786_449_600
        // (verified via Python:
        //  datetime.datetime(2026, 8, 11, 12, 0, 0,
        //  tzinfo=timezone.utc).timestamp() = 1_786_449_600)
        assert_eq!(back.timestamp(), 1_786_449_600);
    }

    #[test]
    fn normalize_transition_matches_chromium_source() {
        assert_eq!(normalize_chromium_transition(0), "link");
        assert_eq!(normalize_chromium_transition(1), "typed");
        assert_eq!(normalize_chromium_transition(2), "reload");
        assert_eq!(normalize_chromium_transition(5), "auto_bookmark");
        assert_eq!(normalize_chromium_transition(20), "redirect");
        // Qualifier bits in the upper byte are ignored.
        assert_eq!(normalize_chromium_transition(0x0100), "link");
        assert_eq!(normalize_chromium_transition(0xFFFF), "other");
    }

    #[test]
    fn read_chromium_filters_to_today_via_synth() {
        // Two URLs: one visited today, one yesterday.
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("History");
        let today_micros = webkit_micros(2026, 8, 11, 12, 0, 0);
        let yesterday_micros = webkit_micros(2026, 8, 10, 12, 0, 0);
        build_chromium_db(
            &db,
            &[
                ("https://today/", "Today", 3, 1, 1, today_micros),
                ("https://yesterday/", "Yesterday", 1, 0, 0, yesterday_micros),
            ],
        );

        let rows = read_chromium(&db, Browser::Chrome, "Default").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 11).unwrap();
        let payload = synthesize(&rows, today, chrono::Utc::now()).unwrap();
        let entries = payload["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "yesterday row dropped");
        assert_eq!(entries[0]["url"], "https://today/");
        assert_eq!(entries[0]["transition_type"], "typed");
        assert_eq!(entries[0]["typed_count"], 1);
        assert_eq!(entries[0]["browser"], "chrome");
        assert_eq!(entries[0]["profile"], "Default");
    }
}