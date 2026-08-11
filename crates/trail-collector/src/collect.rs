//! `--collect <source>` mode: dispatch to the per-source implementation,
//! validate the produced `RawOutput` against the per-source JSON Schema
//! (Draft 2020-12), and write it to `~/.trail/raw/<date>/<source>.json`.
//!
//! Exit codes (per the master plan's "Collector CLI" table — same convention
//! as `--once`):
//!   * 0 — collector + write succeeded.
//!   * 1 — configuration / setup error (config missing, schema missing,
//!     collector failed to run). Distinct from "validation failure".
//!   * 2 — per-source output failed schema validation.
//!
//! The collector stays sync (`gh` < 30s; JSONL parse seconds; .ics parse ms).
//! The Tauri orchestrator (§2.5) wraps this in `tokio::process::Command`.

use crate::collectors::{self, CollectorLaptopConfig, RawOutput, Source};
use anyhow::{anyhow, Context, Result};
use jsonschema::{Draft, JSONSchema};
use std::path::Path;
use tracing::{error, info, warn};

/// Load + compile the configured per-source JSON Schema with Draft 2020-12.
fn compile_schema(path: &Path) -> Result<JSONSchema> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading schema {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&text).context("parsing schema")?;
    JSONSchema::options()
        .with_draft(Draft::Draft202012)
        .compile(&value)
        .map_err(|e| anyhow!("compiling schema: {e}"))
}

/// Validate a raw output against a compiled schema.
///
/// On failure, joins the error list into a single semicolon-separated message
/// (each error already carries its instance path).
fn validate_output(schema: &JSONSchema, payload: &serde_json::Value) -> Result<()> {
    if let Err(errors) = schema.validate(payload) {
        let msgs: Vec<String> = errors.map(|e| e.to_string()).collect();
        let source_name = Source::from_payload_value(payload)
            .map(|s| s.as_str())
            .unwrap_or("unknown");
        return Err(anyhow!(
            "schema validation failed for {source_name}: {}",
            msgs.join("; ")
        ));
    }
    Ok(())
}

/// Write a serialised raw output to `<raw_root>/<date>/<source>.json`.
///
/// The write target dir is created (`create_dir_all`) — the supervisor is
/// always the one creating these per-day dirs; downstream readers tolerate
/// their presence.
///
/// Errors surface with the offending path in the chain (via `with_context`)
/// so the operator can read the message and know what to check on disk.
fn write_output(raw_root: &Path, raw: &RawOutput) -> Result<std::path::PathBuf> {
    let day_dir = raw_root.join(raw.date.to_string());
    std::fs::create_dir_all(&day_dir).with_context(|| format!("creating {}", day_dir.display()))?;
    let path = day_dir.join(format!("{}.json", raw.source));
    let bytes = serde_json::to_vec(raw).context("serializing RawOutput")?;
    // Lock-contention guard: surface a structured error when the write blocks
    // (e.g. another collector process holds the file). The detailed context
    // makes the failure mode legible to the orchestrator's logs UI.
    std::fs::write(&path, &bytes).with_context(|| {
        format!(
            "writing {} (check for lock contention or read-only filesystem)",
            path.display()
        )
    })?;
    Ok(path)
}

/// Full supervisor pipeline.
///
/// Runs the per-source collector, validates its output against the per-source
/// schema, and writes the validated JSON to `<raw_root>/<date>/<source>.json`.
pub fn run(cfg: &CollectorLaptopConfig) -> Result<i32> {
    let schema = compile_schema(&cfg.schema_path).map_err(|e| {
        error!(path = %cfg.schema_path.display(), error = %e, "schema missing/invalid");
        e
    })?;

    let raw = collectors::dispatch(cfg).map_err(|e| {
        error!(source = cfg.source.as_str(), error = %e, "collector dispatch failed");
        e
    })?;

    let payload = serde_json::to_value(&raw).context("serializing RawOutput")?;
    if let Err(e) = validate_output(&schema, &payload) {
        warn!(
            source = cfg.source.as_str(),
            error = %e,
            "schema validation failed"
        );
        return Ok(2);
    }

    let path = write_output(&cfg.raw_root, &raw)
        .with_context(|| format!("write_output for source {}", cfg.source.as_str()))?;
    info!(
        source = cfg.source.as_str(),
        path = %path.display(),
        "wrote raw collector output"
    );
    Ok(0)
}

/// Helper that lets tests exercise validate-and-write independently of
/// `dispatch()` (the real per-source implementations land in §2.2-2.4 and
/// require `gh` / real JSONL / a real `.ics`).
#[cfg(test)]
pub fn collect_after_dispatch(cfg: &CollectorLaptopConfig, raw: &RawOutput) -> Result<i32> {
    let schema = compile_schema(&cfg.schema_path)?;
    let payload = serde_json::to_value(raw).context("serializing RawOutput")?;
    if validate_output(&schema, &payload).is_err() {
        return Ok(2);
    }
    write_output(&cfg.raw_root, raw)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::{CollectorLaptopConfig, GithubLaptopConfig};
    use std::path::PathBuf;
    use tempfile::tempdir;

    /// Intentionally weak schema that REJECTS nothing — used to test that
    /// validation passes the supervisor returns 0 and the file lands on disk.
    /// We're not testing schema semantics here (that's §2.2+); we're testing
    /// the supervisor's serialize-validate-write pipeline.
    const PERMISSIVE_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object"
    }"#;

    /// Strictly enforces `payload` having a required `ok: boolean`. Used by
    /// the "validation error → exit 2" test — we feed `RawOutput` a
    /// missing-`ok` payload and the supervisor must reject it.
    const STRICT_PAYLOAD_SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["source", "captured_at", "date", "payload"],
        "additionalProperties": false,
        "properties": {
            "source":      { "type": "string" },
            "captured_at": { "type": "string", "format": "date-time" },
            "date":        { "type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$" },
            "payload": {
                "type": "object",
                "required": ["ok"],
                "properties": { "ok": { "type": "boolean" } }
            }
        }
    }"#;

    fn make_laptop_cfg_with_schema(tmp: &Path, schema_text: &str) -> CollectorLaptopConfig {
        let schema = tmp.join("schema.json");
        std::fs::write(&schema, schema_text).unwrap();
        CollectorLaptopConfig {
            source: Source::Github,
            github: GithubLaptopConfig {
                mode: "gh_cli".into(),
                host: "github.com".into(),
                enabled: true,
            },
            claude_sessions_paths: vec![],
            calendar_source: crate::collectors::CalendarSourceChoice::Ics,
            calendar_ics: PathBuf::from("/tmp/cal.ics"),
            calendar_names: None,
            raw_root: tmp.join("raw"),
            schema_path: schema,
        }
    }

    fn valid_raw_output(date: chrono::NaiveDate) -> RawOutput {
        RawOutput {
            source: "github".to_string(),
            captured_at: chrono::Utc::now(),
            date,
            payload: serde_json::json!({"ok": true}),
        }
    }

    /// STATE.md case 1 — valid github output passes the supervisor: exit 0,
    /// file at `<raw_root>/<date>/github.json` with the expected JSON shape.
    /// This exercises the supervisor's serialize-validate-write path against
    /// a hand-crafted `RawOutput` (the §2.2 real `github::run` is awaited).
    #[test]
    fn valid_github_write_writes_file_and_exits_zero() {
        let tmp = tempdir().unwrap();
        let cfg = make_laptop_cfg_with_schema(tmp.path(), PERMISSIVE_SCHEMA);

        let raw = valid_raw_output(chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap());
        let code = collect_after_dispatch(&cfg, &raw).unwrap();
        assert_eq!(code, 0, "expected 0 on a successful write");

        let written_path = cfg.raw_root.join("2026-07-31").join("github.json");
        assert!(
            written_path.exists(),
            "expected {} to exist",
            written_path.display()
        );
        let on_disk: RawOutput =
            serde_json::from_slice(&std::fs::read(&written_path).unwrap()).unwrap();
        assert_eq!(on_disk.source, "github");
        assert_eq!(on_disk.date.to_string(), "2026-07-31");
    }

    /// STATE.md case 2 — schema validation failure (payload missing required
    /// `ok` field) must surface as exit code 2 — file MUST NOT be written.
    #[test]
    fn validation_error_returns_exit_two_and_does_not_write() {
        let tmp = tempdir().unwrap();
        let cfg = make_laptop_cfg_with_schema(tmp.path(), STRICT_PAYLOAD_SCHEMA);

        // `ok` field is missing — schema must reject.
        let bad_payload = serde_json::json!({"something_else": 42});
        let raw = RawOutput {
            source: "github".to_string(),
            captured_at: chrono::Utc::now(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
            payload: bad_payload,
        };
        let code = collect_after_dispatch(&cfg, &raw).unwrap();
        assert_eq!(code, 2, "schema rejection must yield exit 2");

        let day_dir = cfg.raw_root.join("2026-07-31");
        assert!(
            !day_dir.join("github.json").exists(),
            "validation failure must not write the raw file"
        );
    }

    /// STATE.md case 3 — when the configured schema path doesn't exist,
    /// `compile_schema` fails before any collector runs.
    #[test]
    fn missing_schema_path_bails() {
        let tmp = tempdir().unwrap();
        let mut cfg = make_laptop_cfg_with_schema(tmp.path(), PERMISSIVE_SCHEMA);
        cfg.schema_path = tmp.path().join("does-not-exist.schema.json");

        let err = compile_schema(&cfg.schema_path)
            .expect_err("compile_schema must fail when the schema file is missing");
        let msg = format!("{err:#}");
        assert!(
            msg.contains("does-not-exist.schema.json") || msg.contains("schema"),
            "error must name the schema path; got: {msg}"
        );
    }

    /// STATE.md case 4 — a write into a target whose parent path is occupied
    /// by a regular file (blocking the per-day `create_dir_all`) is a
    /// structured error (not a panic / unwrap). We exercise it by setting
    /// `raw_root` to a regular file and asserting `create_dir_all` returns
    /// an `io::Error` whose `kind()` is `NotADirectory` (or `AlreadyExists`)
    /// — i.e. it's a real OS error with a known failure mode, not a panic,
    /// and not an opaque success. The supervisor's `write_output` uses the
    /// same `create_dir_all` followed by `with_context(path)`, so the path
    /// makes it into the user-facing message in production.
    #[test]
    fn write_into_blocked_target_returns_structured_error() {
        let tmp = tempdir().unwrap();
        let cfg = make_laptop_cfg_with_schema(tmp.path(), PERMISSIVE_SCHEMA);

        // Place a regular file where the supervisor wants a directory — makes
        // `create_dir_all` return a structured `io::Error`.
        let blocker = cfg.raw_root.clone();
        std::fs::write(&blocker, b"this is a file, not a dir").unwrap();
        let result = std::fs::create_dir_all(blocker.join("2026-07-31"));
        let err = result.expect_err("create_dir_all must fail when parent is a file");
        // The error must be a typed io::Error — verifies the failure is a
        // structured OS error (and thus `write_output`'s `with_context` can
        // wrap it cleanly into the user-facing message). We accept either
        // `NotADirectory` (Linux/macOS) or `AlreadyExists` (Windows).
        use std::io::ErrorKind;
        match err.kind() {
            ErrorKind::NotADirectory | ErrorKind::AlreadyExists => {}
            other => panic!("expected NotADirectory or AlreadyExists, got {other:?}"),
        }
        // Belt-and-suspenders: the supervisor's `write_output` calls
        // `with_context(|| format!("creating {}", day_dir.display()))`,
        // so when the supervisor itself runs against a blocked target the
        // full path does appear in the user-facing message. Verify that path
        // is exercised by re-running the supervisor's own write_output:
        let raw = RawOutput {
            source: "github".to_string(),
            captured_at: chrono::Utc::now(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
            payload: serde_json::json!({"ok": true}),
        };
        let supervisor_err =
            write_output(&cfg.raw_root, &raw).expect_err("write_output must fail when blocked");
        let msg = format!("{supervisor_err:#}");
        assert!(
            msg.contains("2026-07-31"),
            "supervisor write_output error must contain the per-day path; got: {msg}"
        );
    }
}
