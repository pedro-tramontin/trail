//! Phase 3 §3.3 anonymizer — replaces company / project / customer /
//! tool names in the LLM's response with stable placeholders. Three
//! strictness levels: aggressive (default), moderate, off.

use once_cell::sync::Lazy;
use regex::Regex;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AnonymizationStrictness {
    Off,
    Moderate,
    Aggressive,
}

impl FromStr for AnonymizationStrictness {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().as_str() {
            "off" => Self::Off,
            "moderate" => Self::Moderate,
            "aggressive" => Self::Aggressive,
            _ => Self::Aggressive, // default per spec
        })
    }
}

/// A rule: match this literal substring → replace with this placeholder.
/// Applied in `moderate` mode (and always in `aggressive` mode, in
/// addition to the built-in [`AGGRESSIVE_GENERIC_PATTERNS`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AnonymizationRule {
    pub pattern: String,     // a literal substring to find (not a regex)
    pub placeholder: String, // e.g. "[COMPANY]" or "[PROJECT-A]"
}

/// Patterns applied in `aggressive` mode (above and beyond the user's
/// explicit rules). Each is `(regex, placeholder_factory)`. The factory
/// produces a unique placeholder per match (e.g. [COMPANY], [COMPANY-2]).
type AggressivePattern = (&'static str, fn(usize) -> String);
pub static AGGRESSIVE_GENERIC_PATTERNS: Lazy<Vec<AggressivePattern>> = Lazy::new(|| {
    vec![
        // Company suffixes: "ACME Corp", "Foo Inc.", "Bar LLC"
        // The trailing `\.` is optional so we catch "the ACME Corp
        // team" as well as "ACME Corp. published a release".
        (
            r"\b([A-Z][A-Za-z0-9]+(?:\s+[A-Z][A-Za-z0-9]+)*)\s+(?:Inc|Corp|LLC|Ltd|Co|GmbH|SAS|BV|SRL)\b\.?",
            |i| format!("[COMPANY-{i}]"),
        ),
        // Common internal-tool codenames (best-effort; not exhaustive)
        (
            r"\b(jira|confluence|notion|linear|asana|trello|airtable)\b",
            |_| "[TOOL]".to_string(),
        ),
        // URLs (any host): replace with [URL]
        (r"https?://[^\s)]+", |_| "[URL]".to_string()),
        // Email addresses
        (
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            |_| "[EMAIL]".to_string(),
        ),
    ]
});

/// Main entrypoint. `strictness` is the string from config ("off" /
/// "moderate" / "aggressive"). `rules` is the user-configured list of
/// `AnonymizationRule`s. Returns the scrubbed string.
pub fn anonymize(input: &str, strictness: &str, rules: &[AnonymizationRule]) -> String {
    let level = AnonymizationStrictness::from_str(strictness).unwrap();
    let mut out = input.to_string();

    // 1. Apply user-configured rules first (always, even in moderate).
    for rule in rules {
        out = out.replace(&rule.pattern, &rule.placeholder);
    }

    // 2. Apply aggressive patterns in `aggressive` mode.
    if level == AnonymizationStrictness::Aggressive {
        for (pattern, placeholder_factory) in AGGRESSIVE_GENERIC_PATTERNS.iter() {
            if let Ok(re) = Regex::new(pattern) {
                let mut counter = 0usize;
                out = re
                    .replace_all(&out, |_caps: &regex::Captures| {
                        counter += 1;
                        placeholder_factory(counter)
                    })
                    .into_owned();
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(pattern: &str, placeholder: &str) -> AnonymizationRule {
        AnonymizationRule {
            pattern: pattern.to_string(),
            placeholder: placeholder.to_string(),
        }
    }

    #[test]
    fn aggressive_replaces_company_with_placeholder() {
        let input = "Worked with ACME Corp on the integration. ACME Corp approved the design.";
        let out = anonymize(input, "aggressive", &[]);
        assert!(
            out.contains("[COMPANY-"),
            "expected [COMPANY-N] placeholder in: {out}"
        );
        assert!(
            !out.contains("ACME Corp"),
            "ACME Corp should be scrubbed from: {out}"
        );
    }

    #[test]
    fn moderate_only_scrubs_explicit_rules() {
        // Without rules: even "ACME Corp" passes through.
        let input = "Worked with ACME Corp on the integration.";
        let out = anonymize(input, "moderate", &[]);
        assert_eq!(out, input, "moderate without rules must be identity");
        // With rules: explicit pattern gets replaced.
        let rules = vec![rule("ACME Corp", "[COMPANY]")];
        let out = anonymize(input, "moderate", &rules);
        assert!(
            out.contains("[COMPANY]"),
            "explicit rule not applied: {out}"
        );
        assert!(
            !out.contains("ACME Corp"),
            "explicit pattern not replaced: {out}"
        );
    }

    #[test]
    fn off_is_identity() {
        let input = "Worked with ACME Corp on the integration. See https://example.com/x.";
        let out = anonymize(input, "off", &[]);
        assert_eq!(out, input, "off must be identity");
    }

    #[test]
    fn rules_from_config_are_honored() {
        let input = "Working on Project Athena with ACME Corp. Email me at pedro@example.com.";
        let rules = vec![
            rule("Project Athena", "[PROJECT-A]"),
            rule("ACME Corp", "[COMPANY]"),
        ];
        // Aggressive + user rules: both user rules + email URL scrub.
        let out = anonymize(input, "aggressive", &rules);
        assert!(
            out.contains("[PROJECT-A]"),
            "user rule 1 not applied: {out}"
        );
        assert!(out.contains("[COMPANY]"), "user rule 2 not applied: {out}");
        assert!(
            out.contains("[EMAIL]"),
            "aggressive email scrub not applied: {out}"
        );
    }
}
