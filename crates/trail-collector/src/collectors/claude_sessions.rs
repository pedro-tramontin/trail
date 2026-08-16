//! `claude_sessions` collector: walks the configured Claude Code project
//! directories, parses each `*.jsonl` session log, and captures the LAST
//! message per session per day.
//!
//! This module owns the I/O and the directory walking; the pure
//! JSONL→payload transform lives in `synth_claude.rs` next door so the
//! transform is unit-testable without any on-disk fixtures. The
//! collector stays sync (seconds at most for a small project dir); the
//! Tauri orchestrator (§2.5) wraps it in `tokio::process::Command` if
//! it needs to invoke this from an async context.
//!
//! **Path discovery:** Phase 6 onboarding writes
//! `~/.trail/config.json::claude_sessions_paths` (a list of directories
//! under `~/.claude/projects/` the user has chosen to capture). Phase 2
//! reads those paths from the laptop config. An empty list is a valid
//! pre-onboarding state — the collector returns `sessions: []` without
//! touching the filesystem.
//!
//! **Privacy rule (Phase 2 §2.3):** the synthesizer only emits
//! `role`, a ≤280-char `content_headline` of the first text-block,
//! `timestamp`, `session_id`, `project`, and `message_count`. Tool
//! calls, full bodies, image data, and anything under `.local` paths
//! are NEVER captured. The `.local` skip is enforced below at the
//! directory-walk layer so a leaked path can never sneak content into
//! the raw output.

use super::synth_claude;
use super::{CollectorLaptopConfig, RawOutput};
use anyhow::{Context, Result};
use chrono::{Local, Utc};
use std::collections::HashMap;
use walkdir::WalkDir;

/// Per-directory recursion depth cap. Claude Code's `~/.claude/projects/`
/// tree is at most a few levels deep in practice (project dir → session
/// log file); 4 is generous headroom for a v2 nested layout without
/// turning this into a full filesystem walk.
const MAX_WALK_DEPTH: usize = 4;

/// Top-level entry: walk the configured project directories, parse every
/// `*.jsonl` file, and synthesize the per-day raw payload.
///
/// Empty `claude_sessions_paths` is the pre-onboarding state: we return
/// a valid empty envelope (`sessions: []`) so the supervisor's
/// serialize-validate-write pipeline can still run end-to-end.
pub fn run(cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    let now = Utc::now();
    let today = Local::now().date_naive();

    if cfg.claude_sessions_paths.is_empty() {
        // Pre-onboarding state. The supervisor's schema validator still
        // sees a valid envelope (sessions: []) — no filesystem I/O
        // happens, no error is raised.
        return Ok(RawOutput {
            source: "claude_sessions".to_string(),
            captured_at: now,
            date: today,
            payload: serde_json::json!({ "sessions": [] }),
        });
    }

    let mut jsonl_files: Vec<synth_claude::JsonlFile> = Vec::new();
    for root in &cfg.claude_sessions_paths {
        if !root.exists() {
            // A configured path that no longer exists on disk is a
            // user-config drift case, not a fatal error — the other
            // paths may still be valid. Log it and skip.
            tracing::warn!(
                path = %root.display(),
                "configured claude sessions path missing — skipping"
            );
            continue;
        }
        for entry in WalkDir::new(root)
            .min_depth(1)
            .max_depth(MAX_WALK_DEPTH)
            .follow_links(false)
            .into_iter()
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue, // unreadable entry — skip
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let p = entry.path();
            // Only the file extension check; Claude Code writes
            // `<session-id>.jsonl` per session, but tolerate any other
            // extension by simply not picking it up.
            if p.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // Privacy guard: refuse to read any path whose components
            // include `.local` (per the Phase 2 §2.3 rule). The check
            // runs BEFORE `read_to_string` so a `.local` file's content
            // is never decoded.
            if path_contains_local(p) {
                tracing::warn!(path = %p.display(), "skipping .local path (privacy rule)");
                continue;
            }
            let contents =
                std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?;
            jsonl_files.push((p.to_path_buf(), contents));
        }
    }

    // Build the leaf-dirname → project-name map. For now the project
    // name IS the leaf dir name (e.g. `trail`, `personal`); the
    // indirection is what the Phase 6 wizard's `claude_sessions_paths`
    // schema is shaped for.
    let projects_by_dirname: HashMap<String, String> = cfg
        .claude_sessions_paths
        .iter()
        .filter_map(|p| {
            p.file_name()
                .and_then(|s| s.to_str())
                .map(|s| (s.to_string(), s.to_string()))
        })
        .collect();

    let payload = synth_claude::synthesize(&jsonl_files, &projects_by_dirname, today, now)
        .context("synthesizing claude_sessions payload")?;

    Ok(RawOutput {
        source: "claude_sessions".to_string(),
        captured_at: now,
        date: today,
        payload,
    })
}

/// Returns `true` if any component of `path` is the literal `.local`
/// directory. The check is a per-component string match, not a
/// substring search — so a file *named* `.local` (without the dot being
/// a directory) would NOT match; only a directory component named
/// `.local` does. That's the shape the privacy rule targets: `~/.local/`
/// shell-state that may contain other apps' chat history.
fn path_contains_local(path: &std::path::Path) -> bool {
    path.components().any(|c| {
        c.as_os_str()
            .to_str()
            .map(|s| s == ".local")
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collectors::{CollectorLaptopConfig, GithubLaptopConfig, Source};
    use chrono::NaiveDate;
    use serde_json::Value;
    use std::path::PathBuf;

    // Fixtures and the bundled schema are read at compile time; the
    // non-test build carries no fixture bytes.
    const SCHEMA: &str = include_str!("../../schemas/claude_sessions.schema.json");
    const FIXTURE_1: &str = include_str!("../../tests/fixtures/claude_sessions/sessions.jsonl");
    const FIXTURE_2: &str = include_str!("../../tests/fixtures/claude_sessions/sessions_2.jsonl");

    /// Helper: the leaf-name → project-name map used by the tests. Mirrors
    /// what `run()` would build from a config like
    /// `claude_sessions_paths: [~/.../trail, ~/.../personal]`.
    fn projects() -> std::collections::HashMap<String, String> {
        let mut p = std::collections::HashMap::new();
        p.insert("trail".into(), "trail".into());
        p.insert("personal".into(), "personal".into());
        p
    }

    /// Build the full envelope (the shape the supervisor validates
    /// against the schema) from the synth output. Mirrors the github
    /// test pattern (`run_syn` in `github.rs`).
    fn run_envelope() -> Value {
        let files = vec![
            (
                PathBuf::from("/Users/pedro/work/trail/sessions.jsonl"),
                FIXTURE_1.to_string(),
            ),
            (
                PathBuf::from("/Users/pedro/personal/sessions.jsonl"),
                FIXTURE_2.to_string(),
            ),
        ];
        let today = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let now = Utc::now();
        let payload = synth_claude::synthesize(&files, &projects(), today, now).unwrap();
        serde_json::json!({
            "source":      "claude_sessions",
            "captured_at": now.to_rfc3339(),
            "date":        today.format("%Y-%m-%d").to_string(),
            "payload":     payload,
        })
    }

    /// Test 1 — per-session last-message selection. Fixture 1 has four
    /// rows for `sess-1`; the latest (2026-07-31T10:10:00Z, assistant,
    /// "Variant 2 is fine…") must win. The other session from fixture 2
    /// (`sess-2`) is on a different project, so it shows up in the
    /// envelope too — exactly 2 sessions.
    #[test]
    fn synthesize_keeps_last_message_per_session() {
        let envelope = run_envelope();
        let sessions = envelope["payload"]["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 2, "expected 2 sessions across 2 projects");

        // Sort is by `session_id` so the order is stable. sess-1 first.
        assert_eq!(sessions[0]["session_id"], "sess-1");
        assert_eq!(sessions[0]["last_message"]["role"], "assistant");
        assert_eq!(sessions[0]["project"], "trail");
        assert_eq!(sessions[0]["message_count"], 4);
        let headline = sessions[0]["last_message"]["content_headline"]
            .as_str()
            .unwrap();
        assert!(
            headline.contains("Variant 2 is fine"),
            "headline should come from the latest assistant message; got: {headline}"
        );
        assert_eq!(sessions[0]["last_message"]["at"], "2026-07-31T10:10:00Z");

        // sess-2 from the second fixture.
        assert_eq!(sessions[1]["session_id"], "sess-2");
        assert_eq!(sessions[1]["last_message"]["role"], "assistant");
        assert_eq!(sessions[1]["project"], "personal");
        assert_eq!(sessions[1]["message_count"], 2);
    }

    /// Test 2 — `content_headline` truncation at 280 chars. Synthesizes
    /// a one-message JSONL with a 400-char body and asserts the
    /// resulting headline is exactly 280 chars + a U+2026 ellipsis
    /// (281 Unicode scalar values total). The schema's
    /// `maxLength: 280` counts code points, so the ellipsis pushes the
    /// value past the limit — which means the schema MUST NOT have the
    /// `maxLength` constraint on the headline if we keep the ellipsis.
    /// We chose the schema's wording to match the spec (which says
    /// "headline (≤280 chars)"), and we add the ellipsis as a one-char
    /// overrun that the schema will REJECT. To stay schema-compliant
    /// we truncate HARD at 280 and drop the ellipsis when the body
    /// exceeds the limit. Re-running the test with the corrected
    /// expectation (headline == 280 chars, no ellipsis).
    #[test]
    fn synthesize_truncates_content_headline_at_280_chars() {
        // 400 'a' characters in the first text-block.
        let long_body = "a".repeat(400);
        let jsonl = format!(
            "{{\"sessionId\":\"sess-3\",\"cwd\":\"/x\",\"type\":\"assistant\",\
             \"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{long_body}\"}}]}},\
             \"timestamp\":\"2026-07-31T12:00:00Z\"}}\n"
        );
        let files = vec![(PathBuf::from("/Users/pedro/personal/sessions.jsonl"), jsonl)];
        let today = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let out = synth_claude::synthesize(&files, &projects(), today, Utc::now()).unwrap();
        let sessions = out["sessions"].as_array().unwrap();
        assert_eq!(sessions.len(), 1);
        let h = sessions[0]["last_message"]["content_headline"]
            .as_str()
            .unwrap();
        // The schema enforces `maxLength: 280` on `content_headline`.
        // The synthesizer must therefore cap at exactly 280 chars with
        // no trailing ellipsis — a 400-char body collapses to 280 'a's.
        assert!(
            h.chars().count() <= 280,
            "headline must be <=280 chars to pass the schema's maxLength; got {} chars",
            h.chars().count()
        );
        assert_eq!(h.chars().count(), 280, "expected hard truncation at 280");
        assert!(h.chars().all(|c| c == 'a'));
    }

    /// Test 3 — today-only filter. The fixtures all carry 2026-07-31
    /// timestamps; running with a different `today` must yield zero
    /// sessions; running with the matching `today` yields the expected
    /// count. The boundary is at the day level — a session whose last
    /// message is on 2026-07-31 but `today` is 2026-07-30 is dropped.
    #[test]
    fn synthesize_filters_to_today_only() {
        let files = vec![(PathBuf::from("/tmp/sessions.jsonl"), FIXTURE_1.to_string())];
        let today_wrong = NaiveDate::from_ymd_opt(2020, 1, 1).unwrap();
        let out_wrong =
            synth_claude::synthesize(&files, &projects(), today_wrong, Utc::now()).unwrap();
        assert_eq!(
            out_wrong["sessions"].as_array().unwrap().len(),
            0,
            "sessions on 2026-07-31 must not appear when today is 2020-01-01"
        );

        let today_right = NaiveDate::from_ymd_opt(2026, 7, 31).unwrap();
        let out_right =
            synth_claude::synthesize(&files, &projects(), today_right, Utc::now()).unwrap();
        assert_eq!(out_right["sessions"].as_array().unwrap().len(), 1);
    }

    /// Test 4 — the full envelope validates against the bundled schema
    /// (Draft 2020-12). This is the same shape the supervisor's
    /// `compile_schema` will check at runtime; if it passes here, the
    /// `run()` → `RawOutput` → `serde_json::to_value` round-trip is
    /// honest.
    #[test]
    fn synthesize_payload_validates_against_schema() {
        let envelope = run_envelope();
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        let errors: Option<Vec<String>> = {
            let compiled = jsonschema::JSONSchema::options()
                .with_draft(jsonschema::Draft::Draft202012)
                .compile(&schema)
                .unwrap();
            compiled
                .validate(&envelope)
                .err()
                .map(|it| it.map(|e| e.to_string()).collect::<Vec<_>>())
        };
        if let Some(errs) = errors {
            for m in &errs {
                eprintln!("schema error: {m}");
            }
            panic!("envelope failed schema validation: {} error(s)", errs.len());
        }
    }

    /// `run()` with an empty `claude_sessions_paths` returns a valid
    /// `sessions: []` envelope. This is the pre-onboarding state the
    /// Phase 6 wizard relies on. The test piggy-backs on this module so
    /// the synthesizer + the I/O path share coverage.
    #[test]
    fn run_with_empty_paths_returns_sessions_empty() {
        let cfg = CollectorLaptopConfig {
            source: Source::ClaudeSessions,
            github: GithubLaptopConfig {
                mode: "gh_cli".into(),
                host: "github.com".into(),
                enabled: false,
            },
            claude_sessions_paths: vec![],
            calendar_source: crate::collectors::CalendarSourceChoice::Ics,
            calendar_ics: PathBuf::from("/tmp/cal.ics"),
            calendar_names: None,
            raw_root: PathBuf::from("/tmp/raw"),
            schema_path: PathBuf::from("/tmp/schema.json"),
            browser_history: Default::default(),
            // The claude-sessions collector doesn't read remote
            // calendar URLs; the field is empty here.
            remote_calendar_urls: vec![],
        };
        let raw = run(&cfg).expect("run with empty paths must succeed");
        assert_eq!(raw.source, "claude_sessions");
        assert!(raw.payload["sessions"].as_array().unwrap().is_empty());
    }

    /// Privacy: a `.local` path component is skipped at the walk layer.
    /// A configured path under e.g. `~/.local/` must not be read even
    /// if it contains a `.jsonl` file. We exercise the helper directly
    /// (the I/O walk would require a real on-disk fixture).
    #[test]
    fn path_contains_local_detects_dot_local_components() {
        // `~/.local/...` — the `.local` directory component is the target.
        assert!(path_contains_local(std::path::Path::new(
            "/Users/pedro/.local/share/sessions.jsonl"
        )));
        // A `.local` directory nested deeper in a project tree.
        assert!(path_contains_local(std::path::Path::new(
            "/Users/pedro/work/.local/foo.jsonl"
        )));
        // Normal project path: no `.local` component.
        assert!(!path_contains_local(std::path::Path::new(
            "/Users/pedro/work/trail/sessions.jsonl"
        )));
        // A file named `.local.txt` is NOT a `.local` directory — must
        // NOT match. The privacy rule targets `~/.local/`, not arbitrary
        // files whose leaf name contains the string.
        assert!(!path_contains_local(std::path::Path::new(
            "/tmp/.local.txt"
        )));
        // A file named `local` (no dot) is also not a `.local` component.
        assert!(!path_contains_local(std::path::Path::new(
            "/Users/pedro/work/local.jsonl"
        )));
    }
}
