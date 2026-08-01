//! Phase 3 §3.4 learner — classifies user edits to the draft and updates
//! the bootstrap file (`~/.trail/summary_bootstrap.json`) so future
//! summarizer runs see the user's preferences as few-shot context.

use std::path::Path;

use chrono::Utc;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LearningKind {
    AnonymizationCorrection,
    CategorySwap,
    StylePreference,
    InclusionDecision,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BootstrapRule {
    pub kind: LearningKind,
    /// What the user changed (literal substring).
    pub pattern: String,
    /// What they wanted instead.
    pub replacement: String,
    /// How many times this rule has fired.
    pub applied_count: usize,
    /// ISO-8601 timestamp.
    pub last_applied_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SummaryBootstrap {
    /// Schema version; bump on breaking changes.
    pub version: u32,
    pub rules: Vec<BootstrapRule>,
}

impl Default for SummaryBootstrap {
    fn default() -> Self {
        Self {
            version: 1,
            rules: Vec::new(),
        }
    }
}

#[derive(Debug, Error)]
pub enum LearnerError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}

/// Hard cap on the serialized bootstrap file. Exceeding this triggers
/// LRU eviction (see [`compact`]).
pub const BOOTSTRAP_MAX_BYTES: usize = 100 * 1024;

/// Classify a review diff into a [`LearningKind`]. The "diff" is the
/// pair of strings (before-edit, after-edit) for a single section of
/// the draft. Heuristic:
///
/// - if the edit adds/removes a `[PLACEHOLDER]` pattern →
///   [`LearningKind::AnonymizationCorrection`]
/// - if the edit moves content from one `## ` heading to another →
///   [`LearningKind::CategorySwap`]
/// - if the edit is a wording/tense change with no structural change →
///   [`LearningKind::StylePreference`]
/// - if the edit deletes content entirely (a section becomes empty) →
///   [`LearningKind::InclusionDecision`]
/// - otherwise default to [`LearningKind::StylePreference`]
pub fn classify(before: &str, after: &str) -> LearningKind {
    let before_brackets = before.matches('[').count();
    let after_brackets = after.matches('[').count();
    if before_brackets != after_brackets {
        return LearningKind::AnonymizationCorrection;
    }
    if before.trim().is_empty() && !after.trim().is_empty() {
        // Adding content where there was nothing — user wants more of this.
        return LearningKind::InclusionDecision;
    }
    if !before.trim().is_empty() && after.trim().is_empty() {
        return LearningKind::InclusionDecision;
    }
    // Heading change → category swap.
    let before_heading = before.lines().next().unwrap_or("").trim();
    let after_heading = after.lines().next().unwrap_or("").trim();
    if before_heading.starts_with("## ")
        && after_heading.starts_with("## ")
        && before_heading != after_heading
    {
        return LearningKind::CategorySwap;
    }
    LearningKind::StylePreference
}

/// Read the bootstrap file from disk, or return a default empty
/// [`SummaryBootstrap`] if the file is missing.
pub fn load(path: &Path) -> Result<SummaryBootstrap, LearnerError> {
    if !path.exists() {
        return Ok(SummaryBootstrap::default());
    }
    let bytes = std::fs::read(path)?;
    let bootstrap: SummaryBootstrap = serde_json::from_slice(&bytes)?;
    Ok(bootstrap)
}

/// Atomically write the bootstrap file (temp file + rename).
pub fn save(path: &Path, bootstrap: &SummaryBootstrap) -> Result<(), LearnerError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(bootstrap)?;
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

/// Record a single learning event. If the rule already exists (matched
/// by `kind` + `pattern`), increment its `applied_count` and refresh
/// the replacement. Otherwise add a new rule. If the resulting file
/// would exceed [`BOOTSTRAP_MAX_BYTES`], compact via LRU: drop the
/// rule with the lowest `applied_count` (ties broken by oldest
/// `last_applied_at`) until the file fits.
pub fn record_event(
    path: &Path,
    event_kind: LearningKind,
    pattern: &str,
    replacement: &str,
) -> Result<SummaryBootstrap, LearnerError> {
    let mut bootstrap = load(path)?;
    let now = Utc::now().to_rfc3339();
    if let Some(existing) = bootstrap
        .rules
        .iter_mut()
        .find(|r| r.kind == event_kind && r.pattern == pattern)
    {
        existing.applied_count += 1;
        existing.last_applied_at = now.clone();
        // The user's latest choice wins.
        existing.replacement = replacement.to_string();
    } else {
        bootstrap.rules.push(BootstrapRule {
            kind: event_kind,
            pattern: pattern.to_string(),
            replacement: replacement.to_string(),
            applied_count: 1,
            last_applied_at: now,
        });
    }
    compact(&mut bootstrap);
    save(path, &bootstrap)?;
    Ok(bootstrap)
}

/// Drop lowest-applied-count rules until the serialized form fits in
/// [`BOOTSTRAP_MAX_BYTES`]. Ties broken by oldest `last_applied_at`.
fn compact(bootstrap: &mut SummaryBootstrap) {
    loop {
        let bytes = serde_json::to_vec(bootstrap).unwrap_or_default();
        if bytes.len() <= BOOTSTRAP_MAX_BYTES {
            return;
        }
        if bootstrap.rules.is_empty() {
            return;
        }
        // Find the rule with the lowest applied_count; ties broken by oldest last_applied_at.
        let evict_idx = bootstrap
            .rules
            .iter()
            .enumerate()
            .min_by_key(|(_, r)| (r.applied_count, r.last_applied_at.clone()))
            .map(|(i, _)| i)
            .expect("rules is non-empty");
        bootstrap.rules.remove(evict_idx);
    }
}

/// Render the bootstrap as a Markdown block for the LLM prompt. Each
/// rule becomes a bullet: `- When you see \`<pattern>\`, prefer
/// \`<replacement>\` (applied N times)`. Returns `None` if the
/// bootstrap is empty.
pub fn bootstrap_block(path: &Path) -> Result<Option<String>, LearnerError> {
    let bootstrap = load(path)?;
    if bootstrap.rules.is_empty() {
        return Ok(None);
    }
    let mut s = String::new();
    s.push_str("User preferences learned from past review edits:\n");
    for rule in &bootstrap.rules {
        s.push_str(&format!(
            "- When you see `{}`, prefer `{}` (applied {} times)\n",
            rule.pattern, rule.replacement, rule.applied_count
        ));
    }
    Ok(Some(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn classify_anonymization_correction() {
        let before = "Worked with ACME Corp on the integration.";
        let after = "Worked with [COMPANY] on the integration.";
        assert_eq!(
            classify(before, after),
            LearningKind::AnonymizationCorrection
        );
    }

    #[test]
    fn record_event_appends_to_bootstrap_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        let bootstrap = record_event(
            &path,
            LearningKind::AnonymizationCorrection,
            "ACME Corp",
            "[COMPANY]",
        )
        .unwrap();
        assert_eq!(bootstrap.rules.len(), 1);
        assert_eq!(bootstrap.rules[0].applied_count, 1);
        // Idempotent: re-record the same pattern, applied_count goes to 2.
        let bootstrap2 = record_event(
            &path,
            LearningKind::AnonymizationCorrection,
            "ACME Corp",
            "[COMPANY-2]",
        )
        .unwrap();
        assert_eq!(bootstrap2.rules.len(), 1);
        assert_eq!(bootstrap2.rules[0].applied_count, 2);
        // File is on disk.
        assert!(path.exists());
    }

    #[test]
    fn lru_compacts_at_100_kb() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        // Insert 50 rules each with a ~3 KB pattern (well over 100 KB total).
        for i in 0..50 {
            let pattern = format!("pattern-{:02}-{}", i, "x".repeat(2900));
            let _ =
                record_event(&path, LearningKind::StylePreference, &pattern, "[REPLACED]").unwrap();
        }
        let bootstrap = load(&path).unwrap();
        // File should be <= 100 KB.
        let bytes = serde_json::to_vec(&bootstrap).unwrap();
        assert!(
            bytes.len() <= BOOTSTRAP_MAX_BYTES,
            "file size {} > {}",
            bytes.len(),
            BOOTSTRAP_MAX_BYTES
        );
        // We should have evicted at least some rules.
        assert!(
            bootstrap.rules.len() < 50,
            "expected eviction, got {} rules",
            bootstrap.rules.len()
        );
    }

    #[test]
    fn bootstrap_block_renders_markdown_summary() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        record_event(
            &path,
            LearningKind::AnonymizationCorrection,
            "ACME Corp",
            "[COMPANY]",
        )
        .unwrap();
        let block = bootstrap_block(&path).unwrap().unwrap();
        assert!(block.contains("User preferences learned"));
        assert!(block.contains("ACME Corp"));
        assert!(block.contains("[COMPANY]"));
        assert!(block.contains("applied 1 times"));
    }

    #[test]
    fn bootstrap_block_returns_none_for_empty() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        assert!(bootstrap_block(&path).unwrap().is_none());
    }
}
