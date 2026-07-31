//! `--once` mode: process all pending inbox files, append to plan file, move.

use crate::config::CollectorConfig;
use anyhow::{Context, Result};
use jsonschema::JSONSchema;
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::Path;
use tracing::{error, info, warn};

#[derive(Debug, Deserialize)]
struct DaySummary {
    date: String,
    summary: String,
    #[serde(default)]
    wins: Vec<String>,
    #[serde(default)]
    blockers: Vec<String>,
    #[serde(default)]
    people: Vec<String>,
    #[serde(default)]
    open_threads: Vec<String>,
    #[serde(default)]
    voice_notes: Vec<String>,
}

/// Exit codes (per the master plan's "Collector CLI" table):
///   0 = clean run (all files processed, or empty inbox)
///   1 = config error (caller checks this before invoking us)
///   2 = individual file errors (we continue, but report)
pub fn run(cfg: &CollectorConfig) -> Result<i32> {
    // Setup logging to the configured log path.
    setup_logging(&cfg.log_path)?;

    // Load and compile the schema once.
    let schema_text = fs::read_to_string(&cfg.schema_path)
        .with_context(|| format!("reading schema {}", cfg.schema_path.display()))?;
    let schema_value: serde_json::Value =
        serde_json::from_str(&schema_text).context("parsing schema")?;
    let schema = JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .compile(&schema_value)
        .map_err(|e| anyhow::anyhow!("compiling schema: {e}"))?;

    // Discover pending files.
    let inbox = &cfg.inbox_dir;
    if !inbox.is_dir() {
        anyhow::bail!("inbox_dir is not a directory: {}", inbox.display());
    }
    let mut pending: Vec<std::path::PathBuf> = fs::read_dir(inbox)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    pending.sort();

    if pending.is_empty() {
        info!(inbox = %inbox.display(), "no pending files");
        return Ok(0);
    }

    let mut exit_code = 0;
    for file in pending {
        match process_one(cfg, &schema, &file) {
            Ok(()) => info!(file = %file.display(), "processed"),
            Err(e) => {
                warn!(file = %file.display(), error = %e, "quarantining file to failed_dir");
                if let Err(move_err) = move_to(&file, &cfg.failed_dir) {
                    error!(file = %file.display(), error = %move_err, "failed to move file to failed_dir");
                }
                exit_code = 2;
            }
        }
    }
    Ok(exit_code)
}

fn process_one(cfg: &CollectorConfig, schema: &JSONSchema, file: &Path) -> Result<()> {
    let raw = fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let payload: serde_json::Value = serde_json::from_str(&raw)
        .with_context(|| format!("parsing JSON in {}", file.display()))?;
    if let Err(errors) = schema.validate(&payload) {
        let msgs: Vec<String> = errors.map(|e| e.to_string()).collect();
        anyhow::bail!("schema validation failed: {}", msgs.join("; "));
    }
    let day: DaySummary =
        serde_json::from_value(payload).context("deserializing validated payload as DaySummary")?;
    append_to_plan(&cfg.plan_root, &cfg.plan_template, &day)?;
    move_to(file, &cfg.processed_dir)?;
    Ok(())
}

fn append_to_plan(plan_root: &Path, template: &str, day: &DaySummary) -> Result<()> {
    fs::create_dir_all(plan_root)
        .with_context(|| format!("creating plan_root {}", plan_root.display()))?;
    let plan_file = plan_root.join(template.replace("{date}", &day.date));
    let section = render_section(day);

    let needs_header = !plan_file.exists() || fs::metadata(&plan_file)?.len() == 0;
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&plan_file)
        .with_context(|| format!("opening plan file {}", plan_file.display()))?;
    if needs_header {
        writeln!(f, "# {}\n", day.date)?;
    } else {
        writeln!(f, "\n---\n")?;
    }
    f.write_all(section.as_bytes())?;
    Ok(())
}

fn render_section(day: &DaySummary) -> String {
    let mut out = String::new();
    out.push_str(&format!("## Summary\n\n{}\n", day.summary));
    if !day.wins.is_empty() {
        out.push_str("\n## Wins\n");
        for w in &day.wins {
            out.push_str(&format!("- {w}\n"));
        }
    }
    if !day.blockers.is_empty() {
        out.push_str("\n## Blockers\n");
        for b in &day.blockers {
            out.push_str(&format!("- {b}\n"));
        }
    }
    if !day.people.is_empty() {
        out.push_str("\n## People worked with\n");
        for p in &day.people {
            out.push_str(&format!("- {p}\n"));
        }
    }
    if !day.open_threads.is_empty() {
        out.push_str("\n## Open threads\n");
        for t in &day.open_threads {
            out.push_str(&format!("- {t}\n"));
        }
    }
    if !day.voice_notes.is_empty() {
        out.push_str("\n## Voice notes\n");
        for v in &day.voice_notes {
            out.push_str(&format!("- {v}\n"));
        }
    }
    out
}

fn move_to(src: &Path, dest_dir: &Path) -> Result<()> {
    fs::create_dir_all(dest_dir)
        .with_context(|| format!("creating dest dir {}", dest_dir.display()))?;
    let dest = dest_dir.join(
        src.file_name()
            .unwrap_or_else(|| std::ffi::OsStr::new("unknown")),
    );
    fs::rename(src, &dest)
        .with_context(|| format!("moving {} to {}", src.display(), dest.display()))?;
    Ok(())
}

fn setup_logging(log_path: &Path) -> Result<()> {
    use tracing_subscriber::EnvFilter;
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent).ok();
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)
        .with_context(|| format!("opening log file {}", log_path.display()))?;
    // `.try_init()` returns Err if a subscriber is already set; that's fine
    // for our purposes (e.g. a second --once invocation in the same test).
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_writer(file)
        .with_ansi(false)
        .try_init();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

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

    const VALID: &str = r#"{
        "date": "2026-07-31",
        "summary": "Worked on the trail design.",
        "wins": ["Phase D parameterized", "Schema frozen"],
        "blockers": ["Tauri 2 on headless host"],
        "people": ["colleague-A"],
        "open_threads": ["v2 plugin system"],
        "voice_notes": []
    }"#;

    fn setup(tmp: &std::path::Path) -> CollectorConfig {
        let schema = tmp.join("schema.json");
        fs::write(&schema, SCHEMA).unwrap();
        for key in ["inbox", "processed", "failed", "plans"] {
            fs::create_dir_all(tmp.join(key)).unwrap();
        }
        CollectorConfig {
            inbox_dir: tmp.join("inbox"),
            processed_dir: tmp.join("processed"),
            failed_dir: tmp.join("failed"),
            plan_root: tmp.join("plans"),
            plan_template: "{date}.md".to_string(),
            schema_path: schema,
            log_path: tmp.join("collector.log"),
            user: "pedro".to_string(),
            schema_validation: "strict".to_string(),
        }
    }

    #[test]
    fn once_empty_inbox_exits_zero() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = setup(tmp.path());
        let code = run(&cfg).unwrap();
        assert_eq!(code, 0);
    }

    #[test]
    fn once_processes_valid_file_inbox_to_processed() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = setup(tmp.path());
        let file = cfg.inbox_dir.join("2026-07-31.json");
        fs::write(&file, VALID).unwrap();
        let code = run(&cfg).unwrap();
        assert_eq!(code, 0);
        assert!(!file.exists(), "file should be moved out of inbox");
        assert!(cfg.processed_dir.join("2026-07-31.json").exists());
        let plan = fs::read_to_string(cfg.plan_root.join("2026-07-31.md")).unwrap();
        assert!(plan.contains("Worked on the trail design."));
        assert!(plan.contains("## Wins"));
        assert!(plan.contains("Phase D parameterized"));
    }

    #[test]
    fn once_quarantines_invalid_file_to_failed() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = setup(tmp.path());
        let file = cfg.inbox_dir.join("bad.json");
        // Drop several required fields; the schema rejects it.
        let bad = r#"{"date":"2026-07-31","summary":"x"}"#;
        fs::write(&file, bad).unwrap();
        let code = run(&cfg).unwrap();
        assert_eq!(code, 2);
        assert!(!file.exists());
        assert!(cfg.failed_dir.join("bad.json").exists());
    }

    #[test]
    fn once_rejects_malformed_json() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = setup(tmp.path());
        let file = cfg.inbox_dir.join("garbage.json");
        fs::write(&file, b"{ not valid json").unwrap();
        let code = run(&cfg).unwrap();
        // Schema validation pipeline treats this as a schema-rejection (failed_dir).
        assert_eq!(code, 2);
        assert!(cfg.failed_dir.join("garbage.json").exists());
        assert!(!file.exists(), "malformed file should not stay in inbox");
    }
}
