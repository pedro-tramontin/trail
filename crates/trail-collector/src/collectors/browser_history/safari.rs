// SPDX-License-Identifier: MIT
//
// browser_history/safari.rs — reads Safari `History.db`.
// macOS-only. Gated to `#[cfg(target_os = "macos")]`. On
// non-macOS the `mod.rs::run` arm simply skips this path —
// Safari is macOS-only anyway so the user can't have picked it
// on Linux/Windows.
//
// **Schema (Safari 14+, Big Sur and later):**
//
//   history_items(id INTEGER PRIMARY KEY, url TEXT, visit_count INTEGER,
//                 -- v.visit_time is in seconds since the Cocoa epoch
//                 -- (2001-01-01 UTC), Apple's NSDate format.
//                 domain_expansion TEXT NULL, last_seen REAL NULL)
//   history_visits(id INTEGER PRIMARY KEY, history_item INTEGER,
//                  visit_time REAL, redirect_source INTEGER NULL,
//                  title TEXT NULL, -- Safari 14+; pre-14 was NULL
//                  -- transition_type is a 4-character code
//                  -- ("iCab" -> "ICAB", etc.). The standard set:
//                  --   "   " (typed/blank)
//                  --   "GOTO" (typed)
//                  --   "BACK" (back button)
//                  --   "FORM" (form post)
//                  --   "RELD" (reload)
//                  --   "FRAM" (subframe)
//                  --   "LINK" (link click)
//                  --   "BOOK" (bookmark)
//                  --   "AUTO" (auto-subframe)
//                  --   "AUTO_BK" (auto-bookmark)
//                  --   "DOWN" (download)
//                  --   "REDR" (redirect)
//                  --   "GENR" (generated, e.g. URL bar suggest)
//                  --   "CALC" (JavaScript location.* )
//                  --   "CNNT" (connected click — touch/pointer)
//                  --   "ADDB" (add to bookmarks)
//                  transition_type TEXT)
//
// **Timestamps:** Safari uses the Cocoa epoch (2001-01-01 UTC),
// 978307200 seconds before the Unix epoch.

#![cfg(target_os = "macos")]

use anyhow::{Context, Result};
use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OpenFlags};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;

use super::super::synth_browser_history::{Browser, RawHistoryRow};

/// Read Safari's `History.db`. The scanner already verified the
/// file exists and that Full Disk Access is granted (no TCC
/// prompt in this collector — TCC is enforced at the scanner
/// layer which gates whether Safari is in the user's
/// `enabled_browsers` list in the first place).
pub fn read_safari(db_path: &Path) -> Result<Vec<RawHistoryRow>> {
    let temp = copy_to_temp(db_path)
        .with_context(|| format!("copying safari DB {} to temp", db_path.display()))?;
    let conn = Connection::open_with_flags(
        temp.path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("opening safari DB copy at {}", temp.path().display()))?;

    // Per-URL query joining history_items with the most-recent visit.
    let mut stmt = conn.prepare(
        r#"
        SELECT h.url, h.visit_count, v.visit_time, v.title,
               v.transition_type, v.redirect_source
        FROM history_items h
        JOIN history_visits v ON v.id = (
            SELECT id FROM history_visits WHERE history_item = h.id
            ORDER BY visit_time DESC LIMIT 1
        )
        "#,
    )?;

    let mut rows = stmt.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let url: String = r.get(0)?;
        let visit_count: i64 = r.get(1)?;
        let visit_time: f64 = r.get(2)?;
        let title: Option<String> = r.get(3)?;
        let transition_type: Option<String> = r.get(4)?;
        let redirect_source: Option<i64> = r.get(5)?;

        // Cocoa epoch → UTC. visit_time is a REAL (double) so
        // multiply by 1_000_000_000 for nanos; we lose sub-second
        // precision here (Safari's `History.db` only stores whole
        // seconds in practice) so nanos = 0.
        let cocoa_secs = visit_time as i64;
        let last_visit_time = cocoa_secs_to_utc(cocoa_secs);

        let referrer_url = match redirect_source {
            Some(id) if id > 0 => {
                let mut s2 = conn.prepare(
                    "SELECT h.url FROM history_visits v
                     JOIN history_items h ON h.id = v.history_item
                     WHERE v.id = ?1",
                )?;
                s2.query_row([id], |row| row.get::<_, String>(0)).ok()
            }
            _ => None,
        };

        out.push(RawHistoryRow {
            url,
            title: title.unwrap_or_default(),
            last_visit_time,
            visit_count: visit_count.max(0) as u32,
            browser: Browser::Safari,
            profile: "Default".to_string(),
            transition_type: transition_type.as_deref().map(normalize_safari_transition),
            typed_count: None, // Safari doesn't expose this
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
        .unwrap_or("history-db");
    let prefix = format!("trail-{stem}-");
    let temp = tempfile::Builder::new()
        .prefix(&prefix)
        .suffix(".sqlite")
        .tempfile_in(parent)
        .context("creating temp file for safari DB copy")?;
    fs::copy(src, temp.path())
        .with_context(|| format!("copying {} to temp", src.display()))?;
    Ok(temp)
}

fn cocoa_secs_to_utc(cocoa_secs: i64) -> DateTime<Utc> {
    const COCOA_TO_UNIX_SECS: i64 = 978_307_200;
    let unix = cocoa_secs.saturating_add(COCOA_TO_UNIX_SECS);
    Utc.timestamp_opt(unix, 0)
        .single()
        .unwrap_or_else(|| Utc.timestamp_opt(0, 0).unwrap())
}

/// Map Safari's 4-char transition codes to the normalized enum
/// string used in the payload. Unknown codes map to `other` —
/// Safari has added new codes over the years (e.g. Safari 17's
/// `CNNT`) and we don't want a new Safari release to silently
/// drop rows.
fn normalize_safari_transition(code: &str) -> String {
    match code {
        "GOTO" | "   " => "typed",
        "LINK" => "link",
        "BOOK" | "AUTO_BK" => "auto_bookmark",
        "FRAM" | "AUTO" => "auto_subframe",
        "RELD" => "reload",
        "FORM" => "form_submit",
        "REDR" => "redirect",
        "DOWN" | "CALC" | "BACK" | "GENR" | "ADDB" | "CNNT" => "other",
        // Unknown codes (future Safari versions). Don't drop the
        // row — surface it as "other" so the summarizer sees the
        // visit, just without a transition classification.
        _ => "other",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rusqlite::Connection;

    fn build_safari_db(path: &Path, rows: &[(&str, &str, i64, f64, &str, Option<i64>)]) {
        // rows = (url, title, visit_count, visit_time_cocoa, transition_type, redirect_source)
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE history_items (
                id INTEGER PRIMARY KEY,
                url TEXT,
                visit_count INTEGER
            );
            CREATE TABLE history_visits (
                id INTEGER PRIMARY KEY,
                history_item INTEGER,
                visit_time REAL,
                title TEXT,
                transition_type TEXT,
                redirect_source INTEGER
            );
            "#,
        )
        .unwrap();
        for (i, (url, title, vc, vt, t, rs)) in rows.iter().enumerate() {
            let id = (i + 1) as i64;
            conn.execute(
                "INSERT INTO history_items (id, url, visit_count) VALUES (?1, ?2, ?3)",
                rusqlite::params![id, url, vc],
            ).unwrap();
            conn.execute(
                "INSERT INTO history_visits (id, history_item, visit_time, title, transition_type, redirect_source) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![id, id, vt, title, t, rs],
            ).unwrap();
        }
    }

    fn cocoa_secs(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> f64 {
        // Safari stores Cocoa seconds. 2001-01-01 → Unix epoch
        // offset = 978_307_200.
        use chrono::TimeZone;
        let ts = chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap();
        (ts.timestamp() - 978_307_200) as f64
    }

    #[test]
    fn safari_transition_mapping_covers_known_codes() {
        assert_eq!(normalize_safari_transition("GOTO"), "typed");
        assert_eq!(normalize_safari_transition("   "), "typed");
        assert_eq!(normalize_safari_transition("LINK"), "link");
        assert_eq!(normalize_safari_transition("BOOK"), "auto_bookmark");
        assert_eq!(normalize_safari_transition("RELD"), "reload");
        assert_eq!(normalize_safari_transition("REDR"), "redirect");
        assert_eq!(normalize_safari_transition("FORM"), "form_submit");
        assert_eq!(normalize_safari_transition("CNNT"), "other");
        // Future Safari versions — unknown codes default to "other"
        // so the row survives rather than being dropped.
        assert_eq!(normalize_safari_transition("NEW1"), "other");
    }

    #[test]
    fn cocoa_to_utc_offset_is_correct() {
        // 2001-01-01 00:00:00 UTC = Unix 978_307_200
        let t = cocoa_secs_to_utc(0);
        assert_eq!(t.timestamp(), 978_307_200);
    }

    #[test]
    fn read_safari_filters_to_today_via_synth() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("History.db");
        let today_cocoa = cocoa_secs(2026, 8, 11, 12, 0, 0);
        let yesterday_cocoa = cocoa_secs(2026, 8, 10, 12, 0, 0);
        build_safari_db(
            &db,
            &[
                ("https://today.example/", "Today", 2, today_cocoa, "LINK", None),
                ("https://yesterday.example/", "Yesterday", 1, yesterday_cocoa, "GOTO", None),
            ],
        );
        let rows = read_safari(&db).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 11).unwrap();
        let payload = crate::collectors::synth_browser_history::synthesize(
            &rows, today, chrono::Utc::now(),
        ).unwrap();
        let entries = payload["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1, "yesterday row dropped");
        assert_eq!(entries[0]["url"], "https://today.example/");
        assert_eq!(entries[0]["browser"], "safari");
        assert_eq!(entries[0]["transition_type"], "link");
    }
}