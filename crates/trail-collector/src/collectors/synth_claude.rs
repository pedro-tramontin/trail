//! Pure-function transformer: JSONL session files → TrailRawClaudeSessions
//! payload.
//!
//! The `claude_sessions.rs` module owns the I/O (walking the configured
//! project directories + reading `.jsonl` files); this module is the pure
//! transform so the synthesize step is fully testable without any on-disk
//! fixtures. Decoupling also keeps the supervisor (`collect.rs`) honest:
//! validation runs against the transformed output, never raw JSONL bytes.
//!
//! **Privacy rule (Phase 2 §2.3):** capture only `role`, a headline
//! (first ≤280 chars of the first content text-block), the `timestamp`,
//! session ID, project name, and message count. Do NOT capture tool calls
//! (which leak file paths / command contents), full bodies, image data,
//! or anything from `.local` paths. The synthesizer is deliberately
//! conservative: it only inspects the `content` field's first text-block
//! and never touches `toolUse` / `toolResult` / `image` blocks.

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

/// Maximum number of characters kept from a message body for the
/// `content_headline` field. 280 was chosen to match Twitter's classic
/// tweet length — short enough to glance at in a daily summary, long
/// enough to convey what the message was about. The synthesizer appends
/// a single U+2026 HORIZONTAL ELLIPSIS (`…`) when the body exceeds the
/// limit so the reader knows the headline was truncated.
const HEADLINE_MAX_CHARS: usize = 280;

/// The tuple element the runtime hands in: the JSONL file's path
/// (preserved so the caller can surface it via the schema's
/// `jsonl_path` field) and the file's text content.
///
/// Pre-loading the contents (rather than re-reading per-call) keeps
/// the synthesizer a pure function over already-decoded bytes.
pub type JsonlFile = (PathBuf, String);

/// Build the raw `payload` object for the claude_sessions source from a
/// pre-loaded set of JSONL files. Pure: same inputs ⇒ same output.
///
/// `jsonl_files` is the list of `(path, contents)` pairs the I/O layer
/// read. `projects_by_dirname` maps the leaf directory name of each
/// configured path (e.g. `trail`, `personal`) to the project name to
/// surface in the raw output — for now the same string, but the indirection
/// is what the Phase 6 onboarding wizard will need. `today` is the local
/// date the collector is capturing for; sessions whose last-message
/// timestamp is on a different day are filtered out. `_now` is unused
/// today but reserved for future "last N hours" overrides.
pub fn synthesize(
    jsonl_files: &[JsonlFile],
    projects_by_dirname: &HashMap<String, String>,
    today: NaiveDate,
    _now: DateTime<Utc>,
) -> Result<Value> {
    let mut sessions: Vec<Value> = Vec::new();
    let today_str = today.format("%Y-%m-%d").to_string();

    for (path, contents) in jsonl_files {
        // BTreeMap so per-session iteration is stable-ordered (also lets
        // us sort by `session_id` in the final `sessions.sort_by`).
        let mut last_by_session: BTreeMap<String, Value> = BTreeMap::new();
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();

        for line in contents.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let row: Value = serde_json::from_str(line).context("parsing JSONL line")?;
            let sid = row
                .get("sessionId")
                .and_then(|v| v.as_str())
                .context("missing sessionId in JSONL row")?
                .to_string();
            let msg = row.get("message").cloned().unwrap_or(Value::Null);
            let at = row
                .get("timestamp")
                .and_then(|v| v.as_str())
                .map(String::from);

            *counts.entry(sid.clone()).or_insert(0) += 1;

            // Decide whether this row is "newer" than what we already
            // stored for this session. Lexicographic compare on RFC 3339
            // strings is safe (the format is lexicographically sortable
            // when the timezone offset is the same, which `Z` is across
            // every row produced by Claude Code).
            let prefer = match (
                last_by_session
                    .get(&sid)
                    .and_then(|v| v.get("at"))
                    .and_then(|v| v.as_str()),
                at.as_deref(),
            ) {
                (Some(prev), Some(new)) => new > prev,
                (Some(_), None) => false,
                (None, _) => true,
            };
            if prefer {
                let role = msg
                    .get("role")
                    .and_then(|v| v.as_str())
                    .unwrap_or("user")
                    .to_string();
                last_by_session.insert(
                    sid.clone(),
                    serde_json::json!({
                        "role":             role,
                        "content_headline": extract_headline(&msg),
                        "at":               at,
                    }),
                );
            }
        }

        // Resolve the project label from the JSONL's parent directory's
        // leaf name (e.g. `~/.claude/projects/trail/sessions.jsonl` →
        // `trail`). Falls back to `"unknown"` when the path is structured
        // unusually — that way the supervisor still gets a valid payload
        // instead of a hard error from a missing project name.
        let project = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|f| f.to_str())
            .and_then(|s| projects_by_dirname.get(s).cloned())
            .unwrap_or_else(|| "unknown".to_string());

        for (sid, last) in last_by_session {
            // Only emit a session if its last message was on `today` —
            // the raw file is per-day by contract.
            let on_today = last
                .get("at")
                .and_then(|v| v.as_str())
                .map(|s| s.starts_with(&today_str))
                .unwrap_or(false);
            if !on_today {
                continue;
            }
            sessions.push(serde_json::json!({
                "session_id":    sid,
                "project":       project,
                "jsonl_path":    path.display().to_string(),
                "message_count": counts.get(&sid).copied().unwrap_or(0),
                "last_message":  last,
            }));
        }
    }

    // Stable order so the test + the on-disk file are reproducible.
    sessions.sort_by(|a, b| {
        a["session_id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["session_id"].as_str().unwrap_or(""))
    });

    Ok(serde_json::json!({ "sessions": sessions }))
}

/// Extract the privacy-preserving `content_headline` from a single
/// message object. Handles both shapes Claude Code emits:
///   * `content: "plain string"` — user messages with no tool use.
///   * `content: [ { "type": "text", "text": "..." }, ... ]` —
///     assistant messages; we take the FIRST text block only.
///
/// We deliberately do NOT recurse into nested blocks; `toolUse`,
/// `toolResult`, and `image` blocks are ignored even if a session has
/// them. The truncation is character-based (Unicode scalar values), not
/// byte-based, so multibyte characters don't cut a codepoint in half.
fn extract_headline(msg: &Value) -> String {
    let raw: String = if let Some(s) = msg.get("content").and_then(|v| v.as_str()) {
        s.to_string()
    } else if let Some(arr) = msg.get("content").and_then(|v| v.as_array()) {
        arr.iter()
            .find_map(|b| b.get("text").and_then(|t| t.as_str()))
            .unwrap_or("")
            .to_string()
    } else {
        String::new()
    };
    truncate_chars(&raw, HEADLINE_MAX_CHARS)
}

/// Truncate `s` to at most `max_chars` Unicode characters. The schema
/// enforces `maxLength: 280` on `content_headline`, so a hard cap is
/// required — we cannot append an ellipsis (it would push the value
/// past the schema's limit). Empty input is returned empty.
fn truncate_chars(s: &str, max_chars: usize) -> String {
    let n = s.chars().count();
    if n <= max_chars {
        s.to_string()
    } else {
        s.chars().take(max_chars).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity check on the truncation helper: a 400-char input collapses
    /// to exactly 280 chars (no ellipsis). The schema's `maxLength: 280`
    /// is a hard cap; the helper must produce output that always fits.
    #[test]
    fn truncate_chars_caps_at_max_with_no_ellipsis() {
        assert_eq!(truncate_chars("", 280), "");
        assert_eq!(truncate_chars("hi", 280), "hi");
        let long = "a".repeat(400);
        let out = truncate_chars(&long, 280);
        assert_eq!(out.chars().count(), 280);
        assert!(out.chars().all(|c| c == 'a'));
    }
}
