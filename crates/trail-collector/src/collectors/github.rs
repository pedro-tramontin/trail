//! github-collector: shell out to `gh`, capture 24h of PR activity, write JSON.
//!
//! This module owns the I/O and the per-PR `gh pr view`/`gh pr view --json
//! commits` invocations; the pure JSON→payload transform lives in
//! `synth_github.rs` next door so the transform is unit-testable without
//! `gh` on PATH. The collector stays sync (a few seconds at most); the
//! Tauri orchestrator (§2.5) wraps it in `tokio::process::Command` if it
//! needs to invoke this from an async context.

use super::synth_github;
use super::{CollectorLaptopConfig, GithubLaptopConfig, RawOutput};
use anyhow::{bail, Context, Result};
use chrono::{DateTime, Local, Utc};
use serde_json::Value;
use std::collections::HashMap;
use std::process::Command;

/// Top-level entry: capture the 24h PR window for the configured GitHub host.
pub fn run(cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    if !cfg.github.enabled {
        bail!("github collector is disabled in config");
    }
    let now = Utc::now();
    let since = now - chrono::Duration::hours(24);
    let until = now;

    let search_json =
        run_gh(&build_search_args(&cfg.github, since, until)).context("gh search prs")?;
    let search: Value = serde_json::from_str(&search_json).context("parsing search JSON")?;

    let items: Vec<Value> = search
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    let mut views: HashMap<u64, Value> = HashMap::new();
    let mut commits_map: HashMap<u64, Value> = HashMap::new();
    for pr in &items {
        let n = pr
            .get("number")
            .and_then(|v| v.as_u64())
            .context("missing number in search item")?;
        let view_json = run_gh(&build_view_args(&cfg.github, n)).context("gh pr view")?;
        let view: Value = serde_json::from_str(&view_json).context("parsing view JSON")?;
        views.insert(n, view);

        let commits_json =
            run_gh(&build_commits_args(&cfg.github, n)).context("gh pr view --json commits")?;
        let commits: Value = serde_json::from_str(&commits_json).context("parsing commits JSON")?;
        commits_map.insert(n, commits);
    }

    let payload = synth_github::synthesize(&search, &views, &commits_map, now, since, until)
        .context("synthesizing github payload")?;

    Ok(RawOutput {
        source: "github".to_string(),
        captured_at: now,
        date: Local::now().date_naive(),
        payload,
    })
}

/// Capture window — fixed at the last 24 hours. Kept inline in `run` today
/// but exposed for tests / future per-source override.
#[allow(dead_code)]
fn capture_window() -> (DateTime<Utc>, DateTime<Utc>, DateTime<Utc>) {
    let now = Utc::now();
    (now, now - chrono::Duration::hours(24), now)
}

/// `--hostname <host>` is only emitted for GitHub Enterprise hosts; against
/// the public `github.com` `gh` infers the host automatically. This keeps the
/// command line minimal for the common case.
fn gh_host_flag(github_cfg: &GithubLaptopConfig) -> Vec<String> {
    if github_cfg.host == "github.com" {
        Vec::new()
    } else {
        vec!["--hostname".into(), github_cfg.host.clone()]
    }
}

fn build_search_args(
    g: &GithubLaptopConfig,
    since: DateTime<Utc>,
    until: DateTime<Utc>,
) -> Vec<String> {
    let mut args = vec!["search".to_string(), "prs".to_string()];
    args.extend(gh_host_flag(g));
    args.extend([
        "--author".to_string(),
        "@me".to_string(),
        "--state".to_string(),
        "all".to_string(),
        "--created".to_string(),
        format!(
            "{}..{}",
            since.format("%Y-%m-%dT%H:%M:%SZ"),
            until.format("%Y-%m-%dT%H:%M:%SZ")
        ),
        "--json".to_string(),
        "number,title,state,url,createdAt,updatedAt,mergedAt,reviews".to_string(),
        "--limit".to_string(),
        "100".to_string(),
    ]);
    args
}

fn build_view_args(g: &GithubLaptopConfig, n: u64) -> Vec<String> {
    let mut args = vec!["pr".to_string(), "view".to_string(), n.to_string()];
    args.extend(gh_host_flag(g));
    args.extend([
        "--json".to_string(),
        "number,title,state,url,createdAt,updatedAt,mergedAt,reviews".to_string(),
    ]);
    args
}

fn build_commits_args(g: &GithubLaptopConfig, n: u64) -> Vec<String> {
    let mut args = vec!["pr".to_string(), "view".to_string(), n.to_string()];
    args.extend(gh_host_flag(g));
    args.extend(["--json".to_string(), "commits".to_string()]);
    args
}

fn run_gh(args: &[String]) -> Result<String> {
    let out = Command::new("gh")
        .args(args)
        .output()
        .with_context(|| format!("spawning gh {:?}", args))?;
    if !out.status.success() {
        bail!(
            "gh exited {}: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    // Fixtures are only needed by the unit tests; embedding them inside the
    // cfg(test) module keeps the non-test build free of dead-code on the
    // `gh search prs --json ...` shapes the runtime `run()` never inspects.
    const SCHEMA: &str = include_str!("../../schemas/github.schema.json");
    const SEARCH_FIXTURE: &str = include_str!("../../tests/fixtures/github/gh_search_author.json");
    const VIEW_FIXTURE: &str = include_str!("../../tests/fixtures/github/gh_prs_view.json");
    const COMMITS_FIXTURE: &str = include_str!("../../tests/fixtures/github/gh_prs_commits.json");

    /// Load the three fixtures into the shapes `synth_github::synthesize`
    /// expects. The view + commits payloads are scoped to PR #142 (matching
    /// the search fixture), which is enough for the synth unit tests.
    fn fixtures() -> (Value, HashMap<u64, Value>, HashMap<u64, Value>) {
        let search: Value = serde_json::from_str(SEARCH_FIXTURE).unwrap();
        let view: Value = serde_json::from_str(VIEW_FIXTURE).unwrap();
        let commits: Value = serde_json::from_str(COMMITS_FIXTURE).unwrap();
        let mut views = HashMap::new();
        let mut commits_map = HashMap::new();
        views.insert(142u64, view.clone());
        commits_map.insert(142u64, commits.clone());
        (search, views, commits_map)
    }

    fn run_syn() -> Value {
        let (search, views, commits) = fixtures();
        let now = Utc::now();
        let payload = synth_github::synthesize(
            &search,
            &views,
            &commits,
            now,
            now - chrono::Duration::hours(24),
            now,
        )
        .unwrap();
        // Wrap the synthesized inner payload in the full envelope the
        // supervisor validates at runtime (`RawOutput` shape), so
        // `schema.Payload` is `properties.payload` and the envelope's
        // required keys (`source`, `captured_at`, `date`) are all present.
        serde_json::json!({
            "source":      "github",
            "captured_at": now.to_rfc3339(),
            "date":        now.date_naive().format("%Y-%m-%d").to_string(),
            "payload":     payload,
        })
    }

    /// Test 1 — state normalization. gh returns `OPEN` / `MERGED`; the raw
    /// payload uses `open` / `merged`. `merged_at` is `null` for OPEN PRs.
    #[test]
    fn synthesize_handles_open_and_merged_prs_with_state_normalization() {
        let prs = run_syn()["payload"]["prs"].as_array().unwrap().clone();
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0]["state"], "open");
        assert_eq!(prs[0]["merged_at"], Value::Null);
        assert_eq!(prs[1]["state"], "merged");
        assert!(prs[1]["merged_at"].is_string());
    }

    /// Test 2 — events extraction. Open PR carries opened + reviewed +
    /// commented; merged PR carries the merged event.
    #[test]
    fn synthesize_extracts_events_opened_reviewed_merged() {
        let out = run_syn();
        let pr0: Vec<&str> = out["payload"]["prs"][0]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect();
        let pr1: Vec<&str> = out["payload"]["prs"][1]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["type"].as_str().unwrap())
            .collect();
        assert!(pr0.contains(&"opened"));
        assert!(pr0.contains(&"reviewed"));
        assert!(pr0.contains(&"commented"));
        assert!(pr1.contains(&"merged"));
    }

    /// Test 3 — commits include SHA + first-line headline only. Privacy:
    /// no body / diff / file paths.
    #[test]
    fn synthesize_includes_commit_message_headlines_only() {
        let cs = run_syn()["payload"]["prs"][0]["commits"]
            .as_array()
            .unwrap()
            .clone();
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0]["sha"], "abc123");
        assert_eq!(cs[0]["message_headline"], "Add welcome screen");
        assert!(cs[0].get("message").is_none());
        assert!(cs[0].get("diff").is_none());
        assert!(cs[0].get("files").is_none());
    }

    /// Test 4 — payload validates against the bundled schema. The
    /// envelope (`source`/`captured_at`/`date`/`payload`) is what the
    /// supervisor's `compile_schema` validator checks at runtime; here we
    /// round-trip the same shape through `jsonschema::JSONSchema`.
    #[test]
    fn synthesize_payload_validates_against_schema() {
        let envelope = run_syn();
        let schema: Value = serde_json::from_str(SCHEMA).unwrap();
        // Drain any validation errors into a Vec before `compiled` drops so
        // the errors iterator (which borrows `compiled` and `envelope`) can
        // outlive the validate call.
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
                eprintln!("schema error: {}", m);
            }
            panic!("envelope failed schema validation: {} error(s)", errs.len());
        }
    }
}
