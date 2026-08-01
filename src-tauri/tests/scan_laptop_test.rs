//! Integration tests for Phase 6 §6.1 — the non-invasive laptop
//! scan. The unit tests in `onboarding::scan::tests` cover the
//! per-collector happy paths; this integration suite exercises the
//! orchestrator as a whole (all 8 candidates, the report shape, and
//! the no-evidence case) so the contract guarantees are visible
//! without dipping into the private modules.
//!
//! Run with:
//!   cargo test --test scan_laptop_test -- --nocapture

use std::path::Path;

use trail_lib::onboarding::scan::{
    CollectorCandidate, CollectorStatus, EvidenceKind, Platform, ScanReport,
    scan_laptop_with_config,
};

/// Build a tempdir-shaped home directory and stage a single mock
/// artifact under `rel` (relative to the tempdir, with parent
/// directories auto-created). Returns the tempdir root so the
/// caller can stage more files before invoking the scan.
struct TempHome(tempfile::TempDir);

impl TempHome {
    fn new() -> Self {
        let td = tempfile::tempdir().expect("tempdir");
        std::env::set_var("HOME", td.path());
        Self(td)
    }
    fn path(&self) -> &Path {
        self.0.path()
    }
    fn touch(&self, rel: &str) {
        let p = self.0.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, b"x").unwrap();
    }
    fn write_config_with(&self, pending_installs: &[&str]) -> std::path::PathBuf {
        let body = format!(
            r#"{{
                "claude_sessions_paths": [],
                "github": {{"mode":"gh_cli","host":"github.com"}},
                "calendar_ics": "x",
                "voice": {{"enabled":false,"hotkey":"x","transcriber":"x","model":"x"}},
                "review_time": "18:00",
                "summarizer": {{"model":"x","model_provider":"local","anonymization_strictness":"aggressive","use_generic_categories":false}},
                "transport": {{"type":"ssh","host":"x","port":22,"user":"u","auth":{{"auth":"public_key","path":"/tmp/x"}},"remote_path":"/tmp/x"}},
                "raw_retention_days": 7,
                "pending_installs": [{}]
            }}"#,
            pending_installs
                .iter()
                .map(|s| format!("\"{s}\""))
                .collect::<Vec<_>>()
                .join(","),
        );
        let p = self.0.path().join(".trail/config.json");
        self.touch(".trail/config.json");
        std::fs::write(&p, body).unwrap();
        p
    }
}

/// Find a candidate by id. Panics if absent so test failures are
/// attributed to a missing probe rather than a wrong assertion.
fn find<'a>(report: &'a ScanReport, id: &str) -> &'a CollectorCandidate {
    report
        .candidates
        .iter()
        .find(|c| c.collector_id == id)
        .unwrap_or_else(|| panic!("missing candidate {id}"))
}

#[test]
fn report_shape_pins_eight_candidates_and_platform() {
    let home = TempHome::new();
    let platform = Platform::Linux;
    let report = scan_laptop_with_config(
        &platform,
        home.path(),
        &home.path().join(".trail/config.json"),
    );
    assert!(
        report.generated_at.timestamp() > 0,
        "generated_at populated"
    );
    // On this CI host (Linux) we always see Platform::Linux, but the
    // test passes on macOS too because the assertion is just
    // "not the Other(...) fallback for some exotic OS".
    match report.platform {
        Platform::Linux | Platform::Macos => {}
        Platform::Other(ref s) => panic!("unexpected Other platform: {s}"),
    }
    assert_eq!(
        report.candidates.len(),
        8,
        "scan must always produce exactly 8 candidates (master plan)"
    );
    let ids: Vec<&str> = report
        .candidates
        .iter()
        .map(|c| c.collector_id.as_str())
        .collect();
    for expected in [
        "github",
        "calendar",
        "claude_sessions",
        "gmail",
        "notes",
        "vscode_extensions",
        "chrome_history",
        "brave_history",
    ] {
        assert!(ids.contains(&expected), "missing {expected} in scan");
    }
}

#[test]
fn github_evidence_from_gh_config_file_in_tempdir() {
    let home = TempHome::new();
    home.touch(".config/gh/hosts.yml");
    // Race-safe: explicit home.
    let platform = Platform::Linux;
    let report = scan_laptop_with_config(
        &platform,
        home.path(),
        &home.path().join(".trail/config.json"),
    );
    let g = find(&report, "github");
    assert_eq!(g.status, CollectorStatus::Available);
    // `gh auth status` may not be logged in on this CI host so we
    // accept either FileExists or CommandExists — both are
    // Spec-§6.1 "Available".
    match &g.evidence {
        EvidenceKind::FileExists { path } => {
            assert!(path.ends_with("hosts.yml"));
        }
        EvidenceKind::CommandExists { binary, .. } => {
            assert_eq!(binary, "gh");
        }
        other => panic!("unexpected github evidence: {other:?}"),
    }
}

#[test]
fn no_collectors_found_on_empty_home() {
    let home = TempHome::new();
    let platform = Platform::Linux;
    let report = scan_laptop_with_config(
        &platform,
        home.path(),
        &home.path().join(".trail/config.json"),
    );
    for c in &report.candidates {
        assert_eq!(
            c.status,
            CollectorStatus::Unavailable,
            "{} should be Unavailable on an empty $HOME",
            c.collector_id
        );
        assert_eq!(c.confidence, 0.0);
    }
}

#[test]
fn already_configured_overrides_available_in_orchestrator() {
    let home = TempHome::new();
    home.touch(".config/gh/hosts.yml");
    home.touch(".claude/projects/work");
    let cfg_path = home.write_config_with(&["github"]);
    let platform = Platform::Linux;
    let report = scan_laptop_with_config(&platform, home.path(), &cfg_path);

    let g = find(&report, "github");
    assert_eq!(g.status, CollectorStatus::AlreadyConfigured);
    assert_eq!(g.confidence, 1.0);

    // claude_sessions wasn't in pending_installs, so it stays Available.
    let c = find(&report, "claude_sessions");
    assert_eq!(c.status, CollectorStatus::Available);
}
