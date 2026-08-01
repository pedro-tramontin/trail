//! Phase 3 §3.4 learner — classifies user edits to the draft and updates
//! the bootstrap file (`~/.trail/summary_bootstrap.json`) so future
//! summarizer runs see the user's preferences as few-shot context.

use std::path::Path;

use chrono::Utc;
use regex::Regex;
use thiserror::Error;

/// Regex that matches a placeholder-style token like `[COMPANY]`,
/// `[REDACTED:foo]`, etc. We use this to disambiguate the
/// anonymization-correction heuristic from incidental `'['` usage
/// in Markdown links (`[text](url)`) or single-letter checklist
/// bullets (`[x]` / `[X]` / `[ ]`).
///
/// Tighter than `matches('[')` (which fires on any bracket). The
/// rules (PR #36 Copilot thread T1):
///   * opening bracket, an uppercase letter, 2+ uppercase letters
///     / digits / `_` after it (so `[COMPANY]`, `[REDACTED_TYPE]`
///     match; single uppercase `[X]` checklist items do NOT),
///   * an optional `:` followed by 1+ alphanumerics / `-` / `_` /
///     lowercase (so `[REDACTED:foo]`, `[REDACTED:my-rule]` match),
///   * then `]`.
///
/// The single-uppercase-letter exclusion matters because Markdown
/// task-list items are often written `[X]` / `[ ]` and would
/// otherwise be misclassified as a placeholder addition /
/// removal.
fn placeholder_token_re() -> &'static Regex {
    use std::sync::OnceLock;
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Outer: 2+ UPPERCASE letters / digits / underscores
        //   after the opening uppercase letter.
        // Inner (after `:`): allow lowercase / dashes so real
        //   placeholder inner names like "foo" or "my-rule" match.
        // The minimum length of 3 outer uppercase chars prevents
        //   single-letter task-list items like `[X]` / `[Y]`
        //   from being misclassified.
        Regex::new(r"\[[A-Z][A-Z0-9_]{2,}(?::[A-Za-z0-9_-]+)?\]")
            .expect("placeholder_token_re: hardcoded regex must compile")
    })
}

/// Escape raw user-supplied `pattern` / `replacement` strings so they
/// can be safely interpolated into the Markdown inline-code span
/// rendered by [`bootstrap_block`].
///
/// Without this, a rule whose `pattern` or `replacement` contains a
/// backtick (e.g. `` `[REDACTED]` ``) or a literal newline would
/// break out of the inline-code span and inject arbitrary Markdown
/// into the LLM prompt — a prompt-injection vector. We replace each
/// backtick with a similar-looking Unicode grave (U+02CB), collapse
/// embedded newlines, and strip the few control characters that
/// could confuse the model.
fn escape_for_inline_code(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            // Backtick → look-alike U+02CB modifier letter grave accent.
            '`' => out.push('\u{02CB}'),
            // Newline / CR / tab → space (collapses any multi-line pattern
            // into a single-line inline-code span).
            '\n' | '\r' | '\t' => out.push(' '),
            // Strip ASCII control characters (excluding space).
            c if c.is_control() => {} // drop
            c => out.push(c),
        }
    }
    out
}

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
/// - if the edit adds/removes a `[PLACEHOLDER]`-style token
///   (uppercase letters/digits, optional `:inner`) →
///   [`LearningKind::AnonymizationCorrection`]. This deliberately
///   excludes Markdown links (`[text](url)`) and checklist bullets
///   (`[x]` / `[ ]`) which would otherwise trip the heuristic.
/// - if the edit moves content from one `## ` heading to another →
///   [`LearningKind::CategorySwap`]
/// - if the edit is a wording/tense change with no structural change →
///   [`LearningKind::StylePreference`]
/// - if the edit deletes content entirely (a section becomes empty) →
///   [`LearningKind::InclusionDecision`]
/// - otherwise default to [`LearningKind::StylePreference`]
pub fn classify(before: &str, after: &str) -> LearningKind {
    // Count placeholder-style tokens (e.g. [COMPANY], [REDACTED:foo])
    // on each side. We deliberately don't count bare `[` characters
    // because Markdown links and checklist bullets would trip the
    // heuristic on unrelated edits.
    let re = placeholder_token_re();
    let before_placeholders = re.find_iter(before).count();
    let after_placeholders = re.find_iter(after).count();
    if before_placeholders != after_placeholders {
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

/// Write the bootstrap file to `path`.
///
/// Atomicity contract:
/// * **Unix**: `std::fs::rename` is atomic and overwrites; we still
///   write the new content to a sibling `<path>.tmp` first so a
///   writer crash mid-write leaves the prior file intact.
/// * **Windows**: `rename` refuses to overwrite, so we first
///   `remove_file(path)` (ignoring `NotFound`) and then `rename`.
///   This means there is a window between the `remove_file` and
///   `rename` where the destination does not exist; a crash in
///   that window will lose the prior bootstrap file. The
///   non-atomicity on Windows is documented behavior (PR #36
///   Copilot thread T2) and will be revisited in a follow-up
///   via a transactional write path (e.g. ReplaceFileW or
///   rename-on-retry); for now the worst-case-loss is bounded
///   to a single bootstrap file, which is rebuildable from the
///   raw data + recent learner events.
pub fn save(path: &Path, bootstrap: &SummaryBootstrap) -> Result<(), LearnerError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let tmp = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(bootstrap)?;
    std::fs::write(&tmp, bytes)?;
    // Best-effort remove of the destination so the rename below works
    // on Windows (where rename refuses to overwrite). Ignore NotFound.
    if let Err(e) = std::fs::remove_file(path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(LearnerError::Io(e));
        }
    }
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
///
/// Uses [`serde_json::to_vec_pretty`] (matching the writer in
/// [`save`]) so the on-disk file size is bounded. Using the compact
/// `to_vec` would measure a smaller payload and let the pretty
/// writer blow past the cap on disk.
fn compact(bootstrap: &mut SummaryBootstrap) {
    loop {
        let bytes = serde_json::to_vec_pretty(bootstrap).unwrap_or_default();
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
///
/// `pattern` and `replacement` are run through
/// [`escape_for_inline_code`] so a literal backtick, newline, or
/// control character in a stored rule cannot break out of the
/// inline-code span and inject Markdown / prompt content.
pub fn bootstrap_block(path: &Path) -> Result<Option<String>, LearnerError> {
    let bootstrap = load(path)?;
    if bootstrap.rules.is_empty() {
        return Ok(None);
    }
    let mut s = String::new();
    s.push_str("User preferences learned from past review edits:\n");
    for rule in &bootstrap.rules {
        let pat = escape_for_inline_code(&rule.pattern);
        let rep = escape_for_inline_code(&rule.replacement);
        s.push_str(&format!(
            "- When you see `{pat}`, prefer `{rep}` (applied {n} times)\n",
            pat = pat,
            rep = rep,
            n = rule.applied_count
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
    fn classify_ignores_markdown_link_brackets() {
        // Markdown links like [text](url) have bare `[` characters
        // that should NOT be classified as anonymization corrections.
        let before = "See [the docs](https://example.com) for details.";
        let after = "Read [the docs](https://example.com) first.";
        assert_eq!(classify(before, after), LearningKind::StylePreference);
    }

    #[test]
    fn classify_ignores_checklist_brackets() {
        // Checklist bullets [x] / [ ] also have bare `[` that should
        // not trip the anonymization heuristic. (PR #36 Copilot
        // thread T1: the previous regex also matched single
        // uppercase `[X]` checklist "checked" markers; the current
        // 3+-char-minimum regex excludes them too.)
        let before = "- [ ] write tests\n- [ ] review";
        let after = "- [x] write tests\n- [ ] review";
        assert_eq!(classify(before, after), LearningKind::StylePreference);
        // And the uppercase [X] variant -- pre-fix this WAS
        // misclassified as AnonymizationCorrection. Now excluded.
        let before = "- [ ] TODO: ship feature\n- [X] DONE: write spec";
        let after = "- [X] TODO: ship feature\n- [ ] DONE: write spec";
        assert_eq!(
            classify(before, after),
            LearningKind::StylePreference,
            "single-uppercase [X] checklist markers must NOT trip the placeholder heuristic",
        );
    }

    /// Lowercase inner names after `:` should still match. The
    /// PR #36 fix widened the inner character class from
    /// `[A-Z0-9_]+` to `[A-Za-z0-9_-]+`. (PR #36 Copilot thread T1.)
    #[test]
    fn classify_matches_lowercase_inner_after_colon() {
        // Adding `[REDACTED:foo]` should fire the heuristic.
        let before = "Some text here.";
        let after = "Now [REDACTED:foo] covers this.";
        assert_eq!(
            classify(before, after),
            LearningKind::AnonymizationCorrection,
        );
        // And `[REDACTED:my-rule]` too.
        let before2 = "Some text here.";
        let after2 = "Now [REDACTED:my-rule] covers this.";
        assert_eq!(
            classify(before2, after2),
            LearningKind::AnonymizationCorrection,
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

    #[test]
    fn bootstrap_block_escapes_backticks_in_pattern_and_replacement() {
        // A stored rule whose pattern contains a literal backtick
        // would (pre-fix) break out of the inline-code span and
        // inject Markdown into the prompt. The escape substitutes a
        // U+02CB look-alike for backticks inside the stored fields
        // so the inline-code span stays intact.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        record_event(
            &path,
            LearningKind::StylePreference,
            "use `find` here",
            "use `grep` here",
        )
        .unwrap();
        let block = bootstrap_block(&path).unwrap().unwrap();
        // The outer markdown delimiters (one backtick before the
        // pattern, one after, repeated for the replacement) are
        // fine and expected. We assert that the *content* between
        // those delimiters contains no raw backticks — the
        // pattern "use `find` here" should have become "use ˋfindˋ
        // here" (ˋ = U+02CB MODIFIER LETTER GRAVE ACCENT).
        //
        // Extract the pattern segment: text between the first two
        // ASCII backticks on the first bullet line.
        let first_bullet = block
            .lines()
            .find(|l| l.starts_with("- "))
            .expect("at least one bullet");
        // Split the bullet on ASCII backticks. Pattern is at
        // index 1, replacement at index 3.
        let segments: Vec<&str> = first_bullet.split('`').collect();
        assert!(
            segments.len() >= 5,
            "expected 4 backtick-delimiters in bullet, got {}: {first_bullet:?}",
            segments.len()
        );
        let pat = segments[1];
        let rep = segments[3];
        assert!(
            !pat.contains('`'),
            "raw backtick leaked into pattern segment: {pat:?}"
        );
        assert!(
            !rep.contains('`'),
            "raw backtick leaked into replacement segment: {rep:?}"
        );
        // The escaped forms must be present.
        assert!(pat.contains("\u{02CB}find\u{02CB}"));
        assert!(rep.contains("\u{02CB}grep\u{02CB}"));
    }

    #[test]
    fn bootstrap_block_collapses_newlines_in_pattern() {
        // A multi-line pattern (from `record_review_diff` storing
        // whole section text) would (pre-fix) break the inline-code
        // span across lines. The escape collapses newlines to spaces.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        record_event(
            &path,
            LearningKind::StylePreference,
            "line1\nline2\nline3",
            "ok",
        )
        .unwrap();
        let block = bootstrap_block(&path).unwrap().unwrap();
        // The pattern segment (between the first two backticks on
        // the bullet line) must not contain a newline.
        let first_bullet = block
            .lines()
            .find(|l| l.starts_with("- "))
            .expect("at least one bullet");
        let pat = first_bullet
            .split('`')
            .nth(1)
            .expect("bullet has at least one inline-code segment");
        assert!(
            !pat.contains('\n'),
            "newline leaked into pattern segment: {pat:?}"
        );
        // All three source lines are now on one line, space-separated.
        assert!(pat.contains("line1 line2 line3"));
    }

    #[test]
    fn compact_uses_pretty_writer_size_not_compact() {
        // The pre-fix compact() measured `to_vec` but save() wrote
        // `to_vec_pretty`. Verify the on-disk file size (post-save)
        // is within the cap even when the pretty form is materially
        // larger than the compact form.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        // Insert enough rules that pretty + compact diverge noticeably.
        for i in 0..80 {
            let pattern = format!("pattern-{:02}-{}", i, "x".repeat(2000));
            let _ = record_event(&path, LearningKind::StylePreference, &pattern, "ok").unwrap();
        }
        // Re-read from disk and measure with to_vec_pretty (what save uses).
        let bytes = std::fs::read(&path).expect("file should exist on disk");
        assert!(
            bytes.len() <= BOOTSTRAP_MAX_BYTES,
            "on-disk pretty bytes {} exceed cap {}",
            bytes.len(),
            BOOTSTRAP_MAX_BYTES
        );
    }

    #[test]
    fn save_overwrites_existing_destination() {
        // On Windows, fs::rename refuses to overwrite an existing
        // file. save() must remove the destination first (ignoring
        // NotFound) so the subsequent rename works. This is a
        // regression test for that flow.
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("summary_bootstrap.json");
        // First write: create the file.
        let mut bootstrap = SummaryBootstrap::default();
        bootstrap.rules.push(BootstrapRule {
            kind: LearningKind::StylePreference,
            pattern: "first".into(),
            replacement: "FIRST".into(),
            applied_count: 1,
            last_applied_at: "2026-08-01T00:00:00Z".into(),
        });
        save(&path, &bootstrap).unwrap();
        assert!(path.exists());
        // Second write to the same path: must succeed (not just
        // bounce on Windows' "destination exists" rename error).
        bootstrap.rules[0].pattern = "second".into();
        save(&path, &bootstrap).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.rules[0].pattern, "second");
    }
}
