//! Phase 6 — end-to-end onboarding walkthrough.
//!
//! Walks the full Phase A → B → C → D flow against (a) fixture
//! filesystem state for Phase A, (b) a wiremock-ed ollama endpoint
//! for Phase B, (c) a temp `~/.trail/config.json` for Phase C, and
//! (d) a spawned `mock-ssh-server` process for Phase D's auto-install
//! path. Asserts that:
//!
//! * Phase A's `ScanReport` has multiple `Available` candidates
//! * Phase B's `OnboardingAnswers` deserialize from a wiremocked
//!   ollama JSON response and carry a populated `question_log`
//! * Phase C writes `config.json` that round-trips through the
//!   frozen `Config` type, plus appends one JSONL audit log line
//! * Phase D's `install_vps_collector(target, dry_run=true)` lands
//!   the install plan in the mock server's inbox
//!
//! Run with:
//!   cargo test --test onboarding_e2e -- --nocapture
//!
//! Headless-host note: this test requires the `mock-ssh-server`
//! binary to exist. The preflight gates in `tests/e2e_onboarding.sh`
//! build it via `cargo build -p mock-ssh-server`. On a fresh
//! checkout, run `cargo build -p mock-ssh-server` once before
//! invoking the test directly.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tempfile::TempDir;
use trail_lib::config::load_config;
use trail_lib::onboarding::answers::OnboardingAnswers;
use trail_lib::onboarding::config_writer::{
    answers_to_config, append_audit_log, write_config, ConfigWriterError,
};
use trail_lib::onboarding::llm::{ask_onboarding_with, AskOptions, DEFAULT_MODEL, REQUEST_TIMEOUT};
use trail_lib::onboarding::scan::{
    scan_laptop_with_config, CollectorCandidate, CollectorStatus, EvidenceKind, Platform,
    ScanReport,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A canonical, schema-matching ollama envelope that the wiremock
/// returns to the `/api/chat` POST. Mirrors the shape used in the
/// `llm.rs` unit tests so a passing integration run proves the same
/// JSON drives the same downstream code path.
fn fixture_envelope() -> serde_json::Value {
    json!({
        "claude_sessions_paths": {
            "selected": true,
            "notes": "/e2e/fake-home/.claude/projects",
            "evidence_refs": ["claude_sessions"]
        },
        "github": {
            "selected": true,
            "notes": "acme/api",
            "evidence_refs": ["github"]
        },
        "calendar_ics": {
            "selected": false,
            "notes": "",
            "evidence_refs": []
        },
        "voice": {
            "selected": false,
            "model": "base",
            "language": "en",
            "notes": "integration test default",
            "evidence_refs": []
        },
        "review_time": {
            "selected": true,
            "cadence": "evening",
            "hour_utc": 18,
            "notes": "evening at 18:00 (the wizard shows 18:00 in the user's local timezone and stores the equivalent UTC hour)",
            "evidence_refs": []
        },
        "summarizer": {
            "selected": true,
            "backend": "ollama",
            "model": "qwen2.5:7b",
            "notes": null,
            "evidence_refs": []
        },
        "transport": {
            "selected": true,
            "method": "tailscale",
            "notes": null,
            "evidence_refs": []
        },
        "question_log": [
            {
                "question": "Which claude_sessions paths to monitor?",
                "reasoning": "scan found .claude/projects fixture",
                "evidence_refs": ["claude_sessions"]
            },
            {
                "question": "Enable the github collector?",
                "reasoning": "gh auth status fixture returned 0",
                "evidence_refs": ["github"]
            }
        ]
    })
}

/// Wrap the envelope JSON in an `OllamaChatResponse` envelope shape
/// exactly how the bundled `llm::call_ollama` expects to find it.
fn ollama_response_envelope(inner: serde_json::Value) -> serde_json::Value {
    json!({
        "model": "qwen2.5:7b",
        "message": {
            "role": "assistant",
            "content": inner.to_string()
        },
        "done": true,
        "done_reason": "stop"
    })
}

/// Build a tempdir-shaped home directory with the fixture filesystem
/// state Phase A expects to see. Stage the named marker files so
/// `scan_laptop_with_config` reports each one `Available`.
struct FakeHome {
    /// Owns the tempdir; dropped at end of scope.
    _dir: TempDir,
    /// Absolute path to the tempdir root.
    root: PathBuf,
    /// Absolute path to `<root>/.trail/config.json`. Empty initially;
    /// the test writes it as part of Phase C.
    config_path: PathBuf,
}

impl FakeHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        // Stamp the home the lib reads from. The lib's
        // `scan_laptop_with_config` takes a path directly so we
        // don't need HOME — but config_writer's audit + the
        // baseline fallback DO honour HOME, so set it here for
        // consistency. Cleared in Drop via TempDir cleanup + the
        // explicit unset below on test exit.
        std::env::set_var("HOME", &root);
        let config_path = root.join(".trail").join("config.json");
        Self {
            _dir: dir,
            root,
            config_path,
        }
    }

    /// Stage `<root>/<rel>` as a non-empty regular file. Creates
    /// parent directories as needed.
    fn touch(&self, rel: &str, contents: &[u8]) {
        let p = self.root.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).expect("create parent");
        }
        std::fs::write(&p, contents).expect("write fixture");
    }

    /// Run the Phase A scan against this fake home.
    fn scan(&self) -> ScanReport {
        scan_laptop_with_config(&Platform::Linux, &self.root, &self.config_path)
    }
}

impl Drop for FakeHome {
    fn drop(&mut self) {
        // Restore HOME to a benign value so a subsequent test in
        // the same process doesn't inherit our tempdir.
        std::env::remove_var("HOME");
    }
}

/// Wait for `predicate` to return true, polling every 50 ms up to
/// `timeout`. Returns the final value of `predicate()` so the
/// caller can attach an assertion. Avoids flakiness from
/// process-scheduling jitter on the mock server's `read_to_end`.
fn wait_for<F: Fn() -> bool>(timeout: Duration, predicate: F) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    predicate()
}

/// Find a candidate by `collector_id`. Panics if absent so test
/// failures attribute cleanly.
fn find_candidate<'a>(report: &'a ScanReport, id: &str) -> &'a CollectorCandidate {
    report
        .candidates
        .iter()
        .find(|c| c.collector_id == id)
        .unwrap_or_else(|| panic!("missing candidate id={id} in ScanReport"))
}

/// Spawn the workspace's `mock-ssh-server` binary with the given
/// inbox + one-shot + ready-file plumbing, then read back the port
/// it actually bound to. Returns the bound port plus a guard for the
/// child process.
fn spawn_mock_ssh_server(inbox_dir: &Path) -> (u16, std::process::Child) {
    let manifest_dir =
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo test");
    let workspace_root = PathBuf::from(manifest_dir)
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let mock_bin = workspace_root
        .join("target")
        .join("debug")
        .join("mock-ssh-server");
    if !mock_bin.exists() {
        panic!(
            "mock-ssh-server binary not found at {}; \
             run `cargo build -p mock-ssh-server` before running this test",
            mock_bin.display()
        );
    }

    let ready_file = std::env::temp_dir().join(format!(
        "trail-e2e-mock-ready-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = std::fs::remove_file(&ready_file);

    let child = std::process::Command::new(&mock_bin)
        .args([
            "--port",
            "0",
            "--inbox",
            &inbox_dir.to_string_lossy(),
            "--ready-file",
            &ready_file.to_string_lossy(),
            "--one-shot",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn mock-ssh-server");

    // Poll the ready file for up to 5s.
    let mut port: Option<u16> = None;
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        if ready_file.is_file() {
            let mut s = String::new();
            if std::fs::File::open(&ready_file)
                .and_then(|mut f| f.read_to_string(&mut s))
                .is_ok()
            {
                if let Some(line) = s.lines().next() {
                    if let Ok(p) = line.trim().parse::<u16>() {
                        port = Some(p);
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let port = port.unwrap_or_else(|| panic!("mock-ssh-server did not write ready-file in time"));
    let _ = std::fs::remove_file(&ready_file);
    (port, child)
}

// ===========================================================================
// The e2e walkthrough
// ===========================================================================

/// Phase A → B → C → D against fixture filesystem state + a
/// wiremock-ed ollama + a spawned `mock-ssh-server`. This is the
/// proof-of-phase test for Phase 6.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn phase_a_through_phase_d_walkthrough() {
    // -- Phase A: scan a fixture home.
    let home = FakeHome::new();
    home.touch(".config/gh/hosts.yml", b"github.com:\n  user: e2e\n");
    home.touch(
        ".claude/projects/work-project/s1.jsonl",
        br#"{"role":"user","content":"fixture"}"#,
    );
    home.touch(
        ".claude/projects/personal/p1.jsonl",
        br#"{"role":"user","content":"fixture"}"#,
    );
    home.touch(
        ".vscode/extensions/anthropic.claude-code/package.json",
        b"{}",
    );
    // A non-empty extension dir without a package.json shouldn't
    // count for the vscode candidate (the probe looks for any
    // `package.json` under the dir); the above satisfies it.

    let report = home.scan();
    assert!(
        report.candidates.len() >= 3,
        "Phase A must produce at least 3 candidates, got {}",
        report.candidates.len()
    );

    // We expect these to be `Available` because we staged the
    // matching fixture file/dir.
    let github = find_candidate(&report, "github");
    assert_eq!(
        github.status,
        CollectorStatus::Available,
        "github should be Available because ~/.config/gh/hosts.yml was staged"
    );
    assert!(
        matches!(
            github.evidence,
            EvidenceKind::FileExists { .. } | EvidenceKind::CommandExists { .. }
        ),
        "github evidence should be FileExists or CommandExists, got {:?}",
        github.evidence
    );

    let claude = find_candidate(&report, "claude_sessions");
    assert_eq!(
        claude.status,
        CollectorStatus::Available,
        "claude_sessions should be Available because ~/.claude/projects/ was staged"
    );

    // -- Phase B: wiremock-ed ollama round-trip.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/chat"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ollama_response_envelope(fixture_envelope())),
        )
        .mount(&server)
        .await;

    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client");
    let opts = AskOptions {
        ollama_url: server.uri(),
        model: DEFAULT_MODEL.to_string(),
        timeout: REQUEST_TIMEOUT,
    };
    let answers: OnboardingAnswers = ask_onboarding_with(&report, &http, opts).await;

    // The wiremock envelope returned 2 question_log entries.
    assert!(
        !answers.question_log.is_empty(),
        "Phase B must produce a non-empty question_log"
    );
    assert_eq!(
        answers.question_log.len(),
        2,
        "wiremock envelope has 2 question_log entries"
    );
    assert!(
        answers.github.is_some(),
        "wiremock envelope set github=true"
    );
    assert!(
        !answers.claude_sessions_paths.is_empty(),
        "wiremock envelope enabled claude_sessions_paths"
    );
    assert_eq!(answers.summarizer.backend, "ollama");
    assert_eq!(answers.transport.method, "tailscale");

    // -- Phase C: write config + audit log + reload.
    let cfg = answers_to_config(&answers, true);
    write_config(&cfg, &home.config_path).expect("write_config");
    assert!(
        home.config_path.exists(),
        "config.json must exist after write"
    );

    // The audit-log path is `<dest>.jsonl` (config_writer's
    // `append_path` appends the .jsonl extension).
    let audit_path = {
        let mut p = home.config_path.as_os_str().to_os_string();
        p.push(".jsonl");
        PathBuf::from(p)
    };
    append_audit_log(&answers, &home.config_path).expect("append_audit_log");

    // Round-trip through the frozen Config type.
    //
    // `Config::review_time` is the `cadence` string from the
    // onboarding answers ("evening" / "morning" / "weekly") —
    // `answers_to_config` projects the cadence verbatim. The
    // legacy "18:00" string is what an older v1 writer (no
    // onboarding) emits; we don't expect it here because we
    // were just driven by the wiremock envelope.
    let loaded = load_config(&home.config_path).expect("load_config");
    assert_eq!(
        loaded.review_time, "evening",
        "Config::review_time is projected from ReviewTimeConfig.cadence"
    );
    assert_eq!(
        loaded.summarizer.model, "qwen2.5:7b",
        "summarizer.model must round-trip from the wiremock answer (summarizer.backend=ollama)"
    );
    assert_eq!(
        loaded.summarizer.model_provider, "local",
        "frozen summarizer.model_provider round-trips"
    );
    assert_eq!(
        loaded.raw_retention_days, 7,
        "frozen raw_retention_days round-trips"
    );
    assert_eq!(
        loaded.transport_method, "tailscale",
        "transport_method projects from answers.transport.method verbatim"
    );

    // Verify the audit log has at least one valid JSONL row.
    let audit_body = std::fs::read_to_string(&audit_path).expect("audit log readable");
    let mut audit_lines = 0usize;
    for line in audit_body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let parsed: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("invalid JSONL audit line: {e}\nline={line}"));
        assert!(
            parsed.get("answers").is_some(),
            "audit line must carry the answers envelope"
        );
        assert!(
            parsed.get("timestamp").is_some(),
            "audit line must carry a timestamp"
        );
        audit_lines += 1;
    }
    assert!(
        audit_lines >= 1,
        "audit log must have at least one row, got {audit_lines}"
    );

    // -- Phase D: install against the mock-ssh-server.
    let inbox = std::env::temp_dir().join(format!(
        "trail-e2e-inbox-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&inbox).expect("create inbox");
    let (port, mut child) = spawn_mock_ssh_server(&inbox);

    let target = trail_lib::install::VpsInstallTarget {
        host: "127.0.0.1".to_string(),
        port,
        user: "vps_user".to_string(),
    };
    let report_install = trail_lib::install::install_vps_collector(target, true)
        .await
        .expect("install_vps_collector dry-run should succeed against mock-ssh-server");
    assert!(
        report_install.ok,
        "install_vps_collector should report ok=true"
    );
    assert_eq!(report_install.dry_run_port, Some(port));

    // Wait for the mock server to write the install-NNN.json file
    // before asserting. The mock-ssh-server writes *after* the
    // client has half-closed the write side, but cross-process
    // scheduling can race the test's read; poll briefly.
    let landed: Option<PathBuf> = if wait_for(Duration::from_secs(2), || {
        std::fs::read_dir(&inbox)
            .map(|rd| {
                rd.flatten()
                    .any(|e| e.file_name().to_string_lossy().starts_with("install-"))
            })
            .unwrap_or(false)
    }) {
        let mut json_files: Vec<PathBuf> = std::fs::read_dir(&inbox)
            .expect("read_dir after landing")
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("install-"))
                    .unwrap_or(false)
            })
            .collect();
        json_files.sort();
        json_files.into_iter().next()
    } else {
        None
    };
    let landed =
        landed.unwrap_or_else(|| panic!("expected at least one install-NNN.json in {inbox:?}"));

    // Validate the JSON payload shape (timestamp + collector_id +
    // payload).
    let body = std::fs::read_to_string(&landed).expect("install JSON readable");
    let parsed: serde_json::Value = serde_json::from_str(&body)
        .unwrap_or_else(|e| panic!("invalid JSON in {}: {e}\nbody={body}", landed.display()));
    assert_eq!(parsed["collector_id"], "vps_collector");
    assert!(parsed["timestamp"].is_string(), "timestamp must be set");
    assert!(
        parsed["payload"].is_string(),
        "payload must be a string (the rendered install plan)"
    );
    let plan_payload = parsed["payload"].as_str().expect("payload is string");
    assert!(
        plan_payload.contains("trail-collector"),
        "install plan should mention the trail-collector binary path"
    );

    // -- Cleanup.
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&inbox);
}

// ===========================================================================
// Smaller, dedicated coverage. Each case pins one Phase so a failure
// points at a specific layer instead of a wall of context.
// ===========================================================================

/// Phase A in isolation. Confirms the scanner detects ≥3 candidates
/// against fixture filesystem state. Standalone so a Phase A regression
/// is easy to attribute.
#[test]
fn phase_a_scan_finds_three_or_more_available_candidates() {
    let home = FakeHome::new();
    home.touch(".config/gh/hosts.yml", b"github.com:\n  user: e2e\n");
    home.touch(".claude/projects/work/s1.jsonl", b"x");
    home.touch(".claude/projects/personal/p1.jsonl", b"x");
    home.touch(".vscode/extensions/someone.someext/package.json", b"{}");

    let report = home.scan();
    let available_count = report
        .candidates
        .iter()
        .filter(|c| matches!(c.status, CollectorStatus::Available))
        .count();
    assert!(
        available_count >= 3,
        "expected ≥3 Available candidates; got {available_count}; \
         candidates: {:?}",
        report
            .candidates
            .iter()
            .map(|c| format!("{}/{:?}", c.collector_id, c.status))
            .collect::<Vec<_>>()
    );
}

/// Phase C in isolation. Confirms `write_config` produces a JSON
/// file at the destination, `append_audit_log` adds one JSONL line,
/// and the reload round-trips the frozen Config fields.
#[test]
fn phase_c_writes_config_and_appends_audit_log_row() {
    let home = FakeHome::new();
    // Build an answers struct from scratch — `Default` has empty
    // question_log which the test asserts is preserved.
    let mut answers = OnboardingAnswers::default();
    answers
        .claude_sessions_paths
        .push(format!("{}/.claude/projects", home.root.display()));
    answers
        .question_log
        .push(trail_lib::onboarding::answers::QuestionLogEntry {
            question: "e2e phase C".to_string(),
            reasoning: "default answers + cli-created log entry".to_string(),
            evidence_refs: vec!["claude_sessions".to_string()],
        });

    let cfg = answers_to_config(&answers, true);
    write_config(&cfg, &home.config_path).expect("write_config");

    append_audit_log(&answers, &home.config_path).expect("append_audit_log");

    // Reload the config via the frozen type — this is the proof that
    // the written file matches the schema.
    let loaded = load_config(&home.config_path).expect("load_config round-trips");
    assert_eq!(
        loaded.review_time, "evening",
        "Config::review_time is projected from ReviewTimeConfig.cadence (\"evening\" by default)"
    );
    assert_eq!(
        loaded.raw_retention_days, 7,
        "frozen default raw_retention_days must round-trip"
    );
    assert_eq!(
        loaded.summarizer.model, "stub",
        "summarizer.backend=stub (default answers) projects model=stub"
    );

    // Audit log: at least one JSONL row, carries the answers envelope.
    let mut p = home.config_path.as_os_str().to_os_string();
    p.push(".jsonl");
    let audit_path = PathBuf::from(p);
    let body = std::fs::read_to_string(&audit_path).expect("audit log readable");
    let lines: Vec<&str> = body.lines().filter(|l| !l.trim().is_empty()).collect();
    assert!(
        !lines.is_empty(),
        "audit log must have at least one JSONL row, body:\n{body}"
    );
    let parsed: serde_json::Value = serde_json::from_str(lines[0])
        .unwrap_or_else(|e| panic!("invalid JSONL audit row: {e}\nrow={}", lines[0]));
    assert!(
        parsed.get("answers").is_some(),
        "audit row must include `answers`"
    );
    let audit_answers: OnboardingAnswers =
        serde_json::from_value(parsed["answers"].clone()).expect("answers envelope deserializes");
    assert_eq!(audit_answers.question_log.len(), 1);
    assert_eq!(audit_answers.question_log[0].question, "e2e phase C");
}

// ===========================================================================
// Sanity-on-struct: an explicit type check for `ConfigWriterError`. The
// lib's tests already cover this, but exercising the error path at the
// integration boundary documents the shape a production caller would
// see.
// ===========================================================================

#[test]
fn phase_c_write_to_unwritable_path_surfaces_io_error() {
    // `/proc/<pid>/nonexistent_dir/config.json` cannot be created
    // (the parent path is read-only). Use `/dev/null/foo` instead —
    // writing under it always fails.
    let bad = PathBuf::from("/dev/null/cannot_write_here/config.json");
    let home = FakeHome::new();
    let cfg = answers_to_config(&OnboardingAnswers::default(), true);
    let res: Result<(), ConfigWriterError> = write_config(&cfg, &bad);
    match res {
        Err(ConfigWriterError::Io(_)) => { /* expected */ }
        Err(other) => panic!("expected ConfigWriterError::Io, got {other:?}"),
        Ok(()) => panic!("expected error writing to {bad:?}, got Ok"),
    }
    // Suppress the unused-var lint if the test ever simplifies.
    let _ = home;
}
