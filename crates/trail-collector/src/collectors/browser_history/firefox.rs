// SPDX-License-Identifier: MIT
//
// browser_history/firefox.rs — reads Firefox `places.sqlite`.
//
// Firefox stores history + bookmarks + form data in a single
// `places.sqlite` per profile. The history rows live in
// `moz_places` (URL metadata) + `moz_historyvisits` (visit
// events). The profile dir is randomly named
// (`xxxxxxxx.default-release` or `xxxxxxxx.default`); the
// scanner in `src-tauri/src/onboarding/scan.rs::scan_firefox_history`
// globs for the first `places.sqlite` under
// `~/Library/Application Support/Firefox/Profiles/` (macOS) or
// `~/.mozilla/firefox/` (Linux).
//
// **Schema (Firefox 90+):**
//
//   moz_places(id INTEGER PRIMARY KEY, url TEXT, title TEXT,
//              visit_count INTEGER, last_visit_date INTEGER)
//              -- last_visit_date is Unix seconds (NOT µs)
//
//   moz_historyvisits(id INTEGER PRIMARY KEY, from_visit INTEGER,
//                     place_id INTEGER, visit_date INTEGER,
//                     transition_type INTEGER)
//                     -- transition_type is a 1-byte enum;
//                     -- Firefox's source mapping differs from
//                     -- Chromium's.
//
// **Timestamps:** Firefox stores `visit_date` / `last_visit_date`
// as Unix seconds (NOT WebKit µs). Conversion is trivial.
//
// **SQLite-open strategy:** same as `chromium.rs` — copy the
// locked DB to a temp file, open read-only, drop on scope exit.

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use super::super::synth_browser_history::{Browser, RawHistoryRow};

/// Read every Firefox `places.sqlite` in `profile_roots`. Each
/// path is expected to be a `places.sqlite` file directly (the
/// scanner already resolved the per-profile glob).
///
/// Errors on individual files are logged and skipped — same
/// posture as `chromium::read_all_chromium`. An empty `Vec`
/// result is fine if the user picked Firefox but no DB is
/// readable.
pub fn read_all_firefox(profile_roots: &[PathBuf]) -> Result<Vec<RawHistoryRow>> {
    let mut all = Vec::new();
    for path in profile_roots {
        // Firefox's profile dir name is the parent of
        // places.sqlite. We use that as the `profile` field so
        // the summarizer can tell `default-release` apart from
        // `dev-edition`.
        let profile = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("default")
            .to_string();
        match read_firefox(path, &profile) {
            Ok(rows) => all.extend(rows),
            Err(e) => tracing::warn!(
                profile = %profile,
                path = %path.display(),
                error = %e,
                "skipping firefox profile (DB unreadable)"
            ),
        }
    }
    Ok(all)
}

fn read_firefox(db_path: &Path, profile: &str) -> Result<Vec<RawHistoryRow>> {
    let temp = copy_to_temp(db_path)
        .with_context(|| format!("copying firefox DB {} to temp", db_path.display()))?;
    let conn = Connection::open_with_flags(
        temp.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening firefox DB copy at {}", temp.path().display()))?;

    // Per-URL query joining moz_places with the user's most-recent
    // visit in moz_historyvisits. Same MAX(id) trick as the Chromium
    // reader — the latest visit per URL wins.
    let mut stmt = conn.prepare(
        r#"
        SELECT p.url, p.title, p.visit_count, p.last_visit_date,
               v.transition_type, v.from_visit
        FROM moz_places p
        JOIN moz_historyvisits v ON v.id = (
            SELECT id FROM moz_historyvisits WHERE place_id = p.id
            ORDER BY visit_date DESC LIMIT 1
        )
        "#,
    )?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let url: String = r.get(0)?;
        let title: String = r.get(1)?;
        let visit_count: i64 = r.get(2)?;
        let last_visit_date: Option<i64> = r.get(3)?;
        let transition_type: i64 = r.get(4)?;
        let from_visit_id: Option<i64> = r.get(5)?;

        // Firefox stores visit_date as Unix seconds. Some rows
        // have NULL last_visit_date (bookmark-only entries that
        // were never visited) — skip them.
        let last_visit_time = match last_visit_date {
            Some(secs) if secs > 0 => unix_secs_to_utc(secs),
            _ => continue,
        };

        let referrer_url = match from_visit_id {
            Some(id) if id > 0 => {
                let mut s2 = conn.prepare(
                    "SELECT p.url FROM moz_historyvisits v
                     JOIN moz_places p ON p.id = v.place_id
                     WHERE v.id = ?1",
                )?;
                s2.query_row([id], |row| row.get::<_, String>(0)).ok()
            }
            _ => None,
        };

        out.push(RawHistoryRow {
            url,
            title,
            last_visit_time,
            visit_count: visit_count.max(0) as u32,
            browser: Browser::Firefox,
            profile: profile.to_string(),
            transition_type: Some(normalize_firefox_transition(transition_type)),
            typed_count: None, // Firefox doesn't track this
            referrer_url,
        });
    }

    Ok(out)
}

fn copy_to_temp(src: &Path) -> Result<NamedTempFile> {
    let parent = src.parent().unwrap_or_else(|| Path::new("."));
    let stem = src
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("places");
    let prefix = format!("trail-{stem}-");
    let temp = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".sqlite")
        .tempfile_in(parent)
        .context("creating temp file for firefox DB copy")?;
    fs::copy(src, temp.path())
        .with_context(|| format!("copying {} to temp", src.display()))?;
    Ok(temp)
}

fn unix_secs_to_utc(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(secs, 0)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap())
}

/// Map a Firefox `transition_type` integer to the normalized
/// enum string used in the payload.
///
/// Firefox's source enum (from
/// toolkit/components/places/PlacesUtils.jsm):
///
///   1 = TRANSITION_LINK
///   2 = TRANSITION_TYPED       (deprecated in newer Firefox;
///                                still emitted)
///   3 = TRANSITION_BOOKMARK
///   4 = TRANSITION_EMBED       (subframe)
///   5 = TRANSITION_REDIRECT_PERMANENT
///   6 = TRANSITION_REDIRECT_TEMPORARY
///   7 = TRANSITION_DOWNLOAD
///   8 = TRANSITION_FRAMED_LINK
///   9 = TRANSITION_RELOAD
///
/// Firefox doesn't track typed_count separately — `transition_type = 2`
/// is the equivalent signal, and we surface it via the
/// `transition_type` field. `typed_count` is `None` for Firefox.
fn normalize_firefox_transition(t: i64) -> String {
    match t {
        1 => "link",
        2 => "typed",
        3 => "auto_bookmark",
        4 => "auto_subframe",
        5 => "redirect",
        6 => "redirect",
        7 => "other",       // download — no payload equivalent
        8 => "manual_subframe",
        9 => "reload",
        _ => "other",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::synth_browser_history::synthesize;
    use chrono::NaiveDate;
    use rusqlite::Connection;

    fn build_firefox_db(path: &Path, rows: &[(&str, &str, i64, i64, i64)]) {
        // rows = (url, title, visit_count, last_visit_unix_secs, transition_type)
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE moz_places (
                id INTEGER PRIMARY KEY,
                url TEXT,
                title TEXT,
                visit_count INTEGER,
                last_visit_date INTEGER
            );
            CREATE TABLE moz_historyvisits (
                id INTEGER PRIMARY KEY,
                from_visit INTEGER,
                place_id INTEGER,
                visit_date INTEGER,
                transition_type INTEGER
            );
            "#,
        )
        .unwrap();
        for (i, (url, title, vc, lv, t)) in rows.iter().enumerate() {
            let id = (i + 1) as i64;
            conn.execute(
                "INSERT INTO moz_places (id, url, title, visit_count, last_visit_date) VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![id, url, title, vc, lv],
            ).unwrap();
            conn.execute(
                "INSERT INTO moz_historyvisits (id, from_visit, place_id, visit_date, transition_type) VALUES (?1, 0, ?2, ?3, ?4)",
                rusqlite::params![id, id, lv, t],
            ).unwrap();
        }
    }

    fn unix_secs(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap().timestamp()
    }

    #[test]
    fn firefox_transition_mapping_matches_source() {
        assert_eq!(normalize_firefox_transition(1), "link");
        assert_eq!(normalize_firefox_transition(2), "typed");
        assert_eq!(normalize_firefox_transition(3), "auto_bookmark");
        assert_eq!(normalize_firefox_transition(9), "reload");
        assert_eq!(normalize_firefox_transition(99), "other");
    }

    #[test]
    fn read_firefox_filters_to_today_via_synth() {
        let dir = tempfile::tempdir().unwrap();
        let profile_dir = dir.path().join("abcdefgh.default-release");
        std::fs::create_dir_all(&profile_dir).unwrap();
        let db = profile_dir.join("places.sqlite");
        let today_secs = unix_secs(2026, 8, 11, 12, 0, 0);
        let yesterday_secs = unix_secs(2026, 8, 10, 12, 0, 0);
        build_firefox_db(
            &db,
            &[
                ("https://today.example/", "Today", 2, today_secs, 1),
                ("https://yesterday.example/", "Yesterday", 1, yesterday_secs, 1),
            ],
        );

        let rows = read_firefox(&db, "abcdefgh.default-release").unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 11).unwrap();
        let payload = synthesize(&rows, today, chrono::Utc::now()).unwrap();
        let entries = payload["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "yesterday row dropped");
        assert_eq!(entries[0]["url"], "https://today.example/");
        assert_eq!(entries[0]["browser"], "firefox");
        assert_eq!(entries[0]["transition_type"], "link");
        assert!(entries[0]["typed_count"].is_null());
        assert_eq!(entries[0]["profile"], "abcdefgh.default-release");
    }

    #[test]
    fn read_all_firefox_handles_unreadable_profile() {
        // One good profile + one missing file. The good one should
        // produce rows; the missing one logs a warning and is
        // skipped.
        let dir = tempfile::tempdir().unwrap();
        let good_dir = dir.path().join("good-profile");
        std::fs::create_dir_all(&good_dir).unwrap();
        let good_db = good_dir.join("places.sqlite");
        let secs = unix_secs(2026, 8, 11, 12, 0, 0);
        build_firefox_db(
            &good_db,
            &[("https://good.example/", "Good", 1, secs, 1)],
        );
        let bad_path = dir.path().join("does-not-exist").join("places.sqlite");
        let rows = read_all_firefox(&[good_db, bad_path]).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://good.example/");
    }
}