//! Phase 4 §4.1 logs backend — 3 Tauri commands for reading / deleting
//! the day's raw collector files.
//!
//! Reads from `~/.trail/raw/<date>/*.json` (written by the Phase 2
//! collectors). The commands return chronological lists, delete a
//! single raw file, and parse the raw JSON. They are pure with
//! respect to a caller-provided `trail_root`, so the unit tests
//! build a `tempfile::TempDir` instead of touching the real
//! `~/.trail/` hierarchy.

use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// A single raw collector file on disk. Returned by [`list_logs`]
/// and rendered as a row in the Logs UI.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    /// The collector name (e.g. `"github"`, `"calendar"`,
    /// `"claude_sessions"`, `"voice"`). Derived from the file
    /// stem — `<name>.json` in `~/.trail/raw/<date>/`.
    pub source: String,
    /// ISO-8601 timestamp from the file's `captured_at` field.
    /// Empty string if the file didn't carry that field.
    pub captured_at: String,
    /// File size in bytes (`std::fs::metadata().len()`).
    pub size_bytes: u64,
    /// Absolute path to the file on disk. The UI uses this as a
    /// deep-link target.
    pub path: String,
    /// The date partition the file lives under, passed through
    /// from the `list_logs(date)` argument.
    pub date: String,
}

/// Error type for the logs module. All variants convert into a
/// `String` at the Tauri command boundary so the frontend can
/// surface a single human-readable message.
#[derive(Debug, Error)]
pub enum LogsError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid date: {0}")]
    InvalidDate(String),
}

/// Parse a `"YYYY-MM-DD"` string into a [`NaiveDate`]. Returns
/// [`LogsError::InvalidDate`] on failure. We deliberately treat
/// this as timezone-naive: the §4 docs stipulate that the day's
/// raw files partition by *local* calendar day, so a `NaiveDate`
/// is the right shape (no `TimeZone` / `Offset` involved).
pub fn parse_date(date: &str) -> Result<NaiveDate, LogsError> {
    NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|_| LogsError::InvalidDate(date.to_string()))
}

/// Read the day's raw JSON files, return a chronologically-sorted
/// `Vec<LogEntry>`. Returns an empty `Vec` if the date directory
/// doesn't exist — missing days are not an error (the UI should
/// show an empty state instead of a toast).
pub fn list_logs(trail_root: &Path, date: &str) -> Result<Vec<LogEntry>, LogsError> {
    let dir = trail_raw_dir(trail_root, date);
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<LogEntry> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let source = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let metadata = std::fs::metadata(&path)?;
        let bytes = std::fs::read(&path)?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        let captured_at = value
            .get("captured_at")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        entries.push(LogEntry {
            source,
            captured_at,
            size_bytes: metadata.len(),
            path: path.to_string_lossy().into_owned(),
            date: date.to_string(),
        });
    }
    // Chronological order: ascending by `captured_at`. Ties (empty
    // strings) keep their insertion order, which is the OS's
    // `read_dir` order — that's stable enough for v1.
    entries.sort_by(|a, b| a.captured_at.cmp(&b.captured_at));
    Ok(entries)
}

/// Delete the raw file for `(date, source)`. Idempotent — if the
/// file doesn't exist, returns `Ok(())`. We don't update any
/// journal/index file in v1; the file just goes away and the next
/// collector run will write a fresh one.
pub fn delete_log(trail_root: &Path, date: &str, source: &str) -> Result<(), LogsError> {
    // Validate the date up front so the caller gets a clear
    // `InvalidDate` instead of a confusing IO error from
    // `read_dir` against a malformed path component.
    let _ = parse_date(date)?;
    let path = trail_raw_file(trail_root, date, source);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(LogsError::Io(e)),
    }
}

/// Read + parse the raw JSON file for `(date, source)`. Returns
/// the parsed `serde_json::Value` so the frontend can render it
/// as it sees fit (pretty-printed, schema-validated, diffed
/// against the draft, …).
pub fn get_raw_json(
    trail_root: &Path,
    date: &str,
    source: &str,
) -> Result<serde_json::Value, LogsError> {
    let _ = parse_date(date)?;
    let path = trail_raw_file(trail_root, date, source);
    let bytes = std::fs::read(&path)?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    Ok(value)
}

/// Path to the raw directory for a given date:
/// `<trail_root>/raw/<date>/`.
fn trail_raw_dir(trail_root: &Path, date: &str) -> PathBuf {
    trail_root.join("raw").join(date)
}

/// Path to a specific raw collector file:
/// `<trail_root>/raw/<date>/<source>.json`.
fn trail_raw_file(trail_root: &Path, date: &str, source: &str) -> PathBuf {
    trail_raw_dir(trail_root, date).join(format!("{source}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Helper: create a temp trail_root with N JSON files for a date.
    fn setup_day(dir: &Path, date: &str, files: &[(&str, &str)]) {
        let day_dir = dir.join("raw").join(date);
        std::fs::create_dir_all(&day_dir).unwrap();
        for (name, json) in files {
            std::fs::write(day_dir.join(format!("{name}.json")), json).unwrap();
        }
    }

    #[test]
    fn list_logs_returns_chronologically_ordered_entries() {
        let tmp = TempDir::new().unwrap();
        // Insertion order is intentionally *not* the chronological
        // order — the sort step must rescue us.
        setup_day(
            tmp.path(),
            "2026-07-29",
            &[
                (
                    "github",
                    r#"{"source":"github","captured_at":"2026-07-29T18:00:00Z","date":"2026-07-29","payload":{}}"#,
                ),
                (
                    "calendar",
                    r#"{"source":"calendar","captured_at":"2026-07-29T17:30:00Z","date":"2026-07-29","payload":{}}"#,
                ),
                (
                    "claude_sessions",
                    r#"{"source":"claude_sessions","captured_at":"2026-07-29T17:45:00Z","date":"2026-07-29","payload":{}}"#,
                ),
            ],
        );
        let entries = list_logs(tmp.path(), "2026-07-29").unwrap();
        assert_eq!(entries.len(), 3);
        // Sorted by captured_at ascending: 17:30 (calendar),
        // 17:45 (claude_sessions), 18:00 (github).
        assert_eq!(entries[0].source, "calendar");
        assert_eq!(entries[0].captured_at, "2026-07-29T17:30:00Z");
        assert_eq!(entries[1].source, "claude_sessions");
        assert_eq!(entries[1].captured_at, "2026-07-29T17:45:00Z");
        assert_eq!(entries[2].source, "github");
        assert_eq!(entries[2].captured_at, "2026-07-29T18:00:00Z");
        // `date` is the arg, passed through.
        for e in &entries {
            assert_eq!(e.date, "2026-07-29");
        }
    }

    #[test]
    fn list_logs_reports_size_and_absolute_path() {
        let tmp = TempDir::new().unwrap();
        setup_day(
            tmp.path(),
            "2026-07-29",
            &[(
                "github",
                r#"{"source":"github","captured_at":"2026-07-29T18:00:00Z","date":"2026-07-29","payload":{}}"#,
            )],
        );
        let entries = list_logs(tmp.path(), "2026-07-29").unwrap();
        assert_eq!(entries.len(), 1);
        // Size is the file's byte length, not the parsed JSON's
        // length — round-tripping through serde can change it.
        assert!(entries[0].size_bytes > 0);
        // Path is the absolute path the caller can deep-link to.
        assert!(entries[0].path.ends_with("github.json"));
        assert!(std::path::Path::new(&entries[0].path).is_absolute());
    }

    #[test]
    fn delete_log_removes_file_and_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        setup_day(
            tmp.path(),
            "2026-07-29",
            &[(
                "github",
                r#"{"source":"github","captured_at":"2026-07-29T18:00:00Z","date":"2026-07-29","payload":{}}"#,
            )],
        );
        let path = tmp.path().join("raw/2026-07-29/github.json");
        assert!(path.exists());
        // First call: actually removes the file.
        delete_log(tmp.path(), "2026-07-29", "github").unwrap();
        assert!(!path.exists());
        // Second call: idempotent — no error, no panic.
        delete_log(tmp.path(), "2026-07-29", "github").unwrap();
    }

    #[test]
    fn get_raw_json_parses_the_file() {
        let tmp = TempDir::new().unwrap();
        setup_day(
            tmp.path(),
            "2026-07-29",
            &[(
                "github",
                r#"{"source":"github","captured_at":"2026-07-29T18:00:00Z","date":"2026-07-29","payload":{"prs":[]}}"#,
            )],
        );
        let value = get_raw_json(tmp.path(), "2026-07-29", "github").unwrap();
        assert_eq!(value["source"], "github");
        assert_eq!(value["captured_at"], "2026-07-29T18:00:00Z");
        assert!(value["payload"]["prs"].is_array());
    }

    #[test]
    fn missing_day_returns_empty_list() {
        let tmp = TempDir::new().unwrap();
        // Date directory doesn't exist; list_logs must return an
        // empty Vec — *not* an error.
        let entries = list_logs(tmp.path(), "2099-01-01").unwrap();
        assert_eq!(entries.len(), 0);
    }

    /// Timezone-naive date parsing matches §4 docs: a bare
    /// `YYYY-MM-DD` string round-trips through `NaiveDate` with
    /// no timezone / offset attached. We pin the year/month/day
    /// fields explicitly so a future switch to a `DateTime`
    /// timezone-aware type is caught here.
    #[test]
    fn parse_date_is_timezone_naive() {
        use chrono::Datelike;
        let d = parse_date("2026-07-29").unwrap();
        assert_eq!(d.year(), 2026);
        assert_eq!(d.month(), 7);
        assert_eq!(d.day(), 29);
    }

    #[test]
    fn parse_date_rejects_malformed_string() {
        assert!(parse_date("not-a-date").is_err());
        assert!(parse_date("2026-13-29").is_err()); // invalid month
        assert!(parse_date("2026-07-32").is_err()); // invalid day
        assert!(parse_date("2026-07-29").is_ok());
    }

    #[test]
    fn get_raw_json_returns_error_for_missing_file() {
        let tmp = TempDir::new().unwrap();
        let result = get_raw_json(tmp.path(), "2026-07-29", "nonexistent");
        assert!(result.is_err());
    }
}
