//! `--validate <file>` mode: schema-check a single file against the configured schema.

use crate::config::CollectorConfig;
use anyhow::{Context, Result};
use jsonschema::JSONSchema;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct ValidateReport {
    pub ok: bool,
    pub file: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Validate `file` against the schema configured in `cfg.schema_path`.
///
/// Returns 0 if the file is valid, 1 if validation or parsing failed.
/// The report (with `ok` + `errors[]`) is always printed to stdout,
/// which lets the Tauri app pre-push validator (item 1-6) parse the
/// outcome without depending on exit codes alone.
pub fn run(cfg: &CollectorConfig, file: &Path) -> Result<i32> {
    let schema_text = std::fs::read_to_string(&cfg.schema_path)
        .with_context(|| format!("reading schema {}", cfg.schema_path.display()))?;
    let schema_value: serde_json::Value =
        serde_json::from_str(&schema_text).context("parsing schema as JSON")?;
    let schema = JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .compile(&schema_value)
        .map_err(|e| anyhow::anyhow!("compiling schema: {e}"))?;

    let payload_text = std::fs::read_to_string(file)
        .with_context(|| format!("reading file {}", file.display()))?;
    let payload: serde_json::Value = match serde_json::from_str(&payload_text) {
        Ok(v) => v,
        Err(e) => {
            let report = ValidateReport {
                ok: false,
                file: file.display().to_string(),
                errors: vec![format!("JSON parse: {e}")],
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
            return Ok(1);
        }
    };

    let result = schema.validate(&payload);
    if let Err(errors) = result {
        // Collect errors as a sorted list of human-readable strings.
        let mut msgs: Vec<String> = errors
            .map(|e| format!("{} at {}", e, e.instance_path))
            .collect();
        msgs.sort();
        let report = ValidateReport {
            ok: false,
            file: file.display().to_string(),
            errors: msgs,
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(1);
    }

    let report = ValidateReport {
        ok: true,
        file: file.display().to_string(),
        errors: vec![],
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;

    // Mirror of `resources/day-summary.schema.json` (item 1-5 master contract).
    // Keep in sync with the bundled schema if either changes.
    const SCHEMA: &str = r#"{
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "DaySummary",
        "type": "object",
        "required": ["date", "summary", "wins", "blockers", "people", "open_threads", "voice_notes"],
        "additionalProperties": false,
        "properties": {
            "date":         {"type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$"},
            "summary":      {"type": "string"},
            "wins":         {"type": "array", "items": {"type": "string"}},
            "blockers":     {"type": "array", "items": {"type": "string"}},
            "people":       {"type": "array", "items": {"type": "string"}},
            "open_threads": {"type": "array", "items": {"type": "string"}},
            "voice_notes":  {"type": "array", "items": {"type": "string"}}
        }
    }"#;

    const VALID_DAY_SUMMARY: &str = r#"{
        "date": "2026-07-31",
        "summary": "Worked on the trail design.",
        "wins": ["Phase D parameterized", "Schema frozen"],
        "blockers": ["Tauri 2 on headless host"],
        "people": ["colleague-A"],
        "open_threads": ["v2 plugin system"],
        "voice_notes": []
    }"#;

    fn setup(tmp: &std::path::Path) -> (CollectorConfig, std::path::PathBuf) {
        let schema = tmp.join("schema.json");
        fs::write(&schema, SCHEMA).unwrap();
        let cfg = CollectorConfig {
            inbox_dir: tmp.join("inbox"),
            processed_dir: tmp.join("processed"),
            failed_dir: tmp.join("failed"),
            plan_root: tmp.join("plans"),
            plan_template: "{date}.md".to_string(),
            schema_path: schema,
            log_path: tmp.join("collector.log"),
            user: "pedro".to_string(),
            schema_validation: "strict".to_string(),
        };
        let file = tmp.join("day.json");
        (cfg, file)
    }

    #[test]
    fn validate_accepts_valid_day_summary() {
        let tmp = tempfile::tempdir().unwrap();
        let (cfg, file) = setup(tmp.path());
        fs::write(&file, VALID_DAY_SUMMARY).unwrap();
        let code = run(&cfg, &file).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        let tmp = tempfile::tempdir().unwrap();
        let (cfg, file) = setup(tmp.path());
        // Drop the `blockers` field.
        let bad = r#"{
            "date": "2026-07-31",
            "summary": "x",
            "wins": [], "people": [], "open_threads": [], "voice_notes": []
        }"#;
        fs::write(&file, bad).unwrap();
        let code = run(&cfg, &file).unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn validate_rejects_bad_date_format() {
        let tmp = tempfile::tempdir().unwrap();
        let (cfg, file) = setup(tmp.path());
        let bad = r#"{
            "date": "31-07-2026",
            "summary": "x",
            "wins": [], "blockers": [], "people": [], "open_threads": [], "voice_notes": []
        }"#;
        fs::write(&file, bad).unwrap();
        let code = run(&cfg, &file).unwrap();
        assert_eq!(code, 1);
    }

    #[test]
    fn validate_handles_invalid_json_gracefully() {
        let tmp = tempfile::tempdir().unwrap();
        let (cfg, file) = setup(tmp.path());
        let mut f = fs::File::create(&file).unwrap();
        f.write_all(b"{ not valid json").unwrap();
        let code = run(&cfg, &file).unwrap();
        assert_eq!(code, 1);
    }
}
