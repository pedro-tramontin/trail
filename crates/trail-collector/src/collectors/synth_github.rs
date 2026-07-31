//! Pure-function transformer: gh --json shapes → TrailRawGithub payload.
//!
//! The `github.rs` module shells out to `gh` for the actual JSON; this module
//! is the pure transform so the synthesize step is fully testable without the
//! `gh` binary on PATH. Decoupling also keeps the supervisor (`collect.rs`)
//! honest: validation runs against the transformed output, never raw `gh`
//! bytes.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::HashMap;

/// Build the raw `payload` object for the github source from the per-PR
/// `gh` JSON shapes. Pure: same inputs ⇒ same output.
///
/// `search` is the JSON returned by `gh search prs --json ...`,
/// `views_by_number` maps PR number → `gh pr view --json ...` payload,
/// `commits_by_number` maps PR number → `gh pr view --json commits ...`
/// payload. `now` / `window_start` / `window_end` are the capture window
/// bounds (the latter two used as `since` / `until` in the wrapped envelope).
pub fn synthesize(
    search: &Value,
    views_by_number: &HashMap<u64, Value>,
    commits_by_number: &HashMap<u64, Value>,
    _now: DateTime<Utc>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Value> {
    let mut prs: Vec<Value> = Vec::new();
    let items: Vec<Value> = search
        .get("items")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    for pr in items {
        let n = pr
            .get("number")
            .and_then(|v| v.as_u64())
            .context("missing PR number in search item")?;
        let state = match pr.get("state").and_then(|v| v.as_str()).unwrap_or("") {
            "OPEN" => "open",
            "CLOSED" => "closed",
            "MERGED" => "merged",
            other => anyhow::bail!("unknown gh state: {other}"),
        };

        let opened_at = pr.get("createdAt").and_then(|v| v.as_str()).unwrap_or("");
        let mut events: Vec<Value> = vec![serde_json::json!({
            "type": "opened",
            "at":   opened_at,
            "by":   Value::Null,
        })];

        if let Some(view) = views_by_number.get(&n) {
            let reviews: Vec<Value> = view
                .get("reviews")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for review in reviews {
                let rstate = review.get("state").and_then(|v| v.as_str()).unwrap_or("");
                let evtype = match rstate {
                    "COMMENTED" => "commented",
                    // APPROVED, CHANGES_REQUESTED, DISMISSED → all "reviewed"
                    _ => "reviewed",
                };
                let at = review
                    .get("submittedAt")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let by = review
                    .get("author")
                    .and_then(|a| a.get("login"))
                    .and_then(|v| v.as_str());
                events.push(serde_json::json!({
                    "type": evtype,
                    "at":   at,
                    "by":   by,
                }));
            }
        }

        let merged_at_value = pr.get("mergedAt").cloned().unwrap_or(Value::Null);
        if !merged_at_value.is_null() {
            events.push(serde_json::json!({
                "type": "merged",
                "at":   merged_at_value,
                "by":   Value::Null,
            }));
        }

        // Privacy: commit SHAs + first-line headlines ONLY. No body, no diff,
        // no file paths.
        let commits: Vec<Value> = commits_by_number
            .get(&n)
            .and_then(|c| c.get("commits"))
            .and_then(|c| c.as_array())
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|c| {
                serde_json::json!({
                    "sha":              c.get("oid").and_then(|v| v.as_str()).unwrap_or(""),
                    "message_headline": c.get("messageHeadline").and_then(|v| v.as_str()).unwrap_or(""),
                })
            })
            .collect();

        prs.push(serde_json::json!({
            "number":     n,
            "title":      pr.get("title").and_then(|v| v.as_str()).unwrap_or(""),
            "state":      state,
            "url":        pr.get("url").and_then(|v| v.as_str()).unwrap_or(""),
            "created_at": opened_at,
            "updated_at": pr.get("updatedAt").and_then(|v| v.as_str()).unwrap_or(""),
            "merged_at":  merged_at_value,
            "events":     events,
            "commits":    commits,
        }));
    }

    Ok(serde_json::json!({
        "author": "@me",
        "since":  window_start.to_rfc3339(),
        "until":  window_end.to_rfc3339(),
        "prs":    prs,
    }))
}
