//! Hardcoded baseline answers used when the local ollama server is
//! unreachable (or returns an error we can't recover from). The
//! baseline is "conservative": it turns on only the collectors whose
//! `ScanReport` evidence is strongest, leaves the GPU-bound ones off
//! (voice), and picks deterministic defaults for the always-on knobs
//! (review_time, summarizer=stub, transport=ssh).
//!
//! The function is `fn`, not `async fn`: the baseline is a pure
//! data transform over the scan, so there's nothing to await. Tests
//! construct a `ScanReport` directly and verify the mapping.

use super::answers::{
    GitHubConfig, OnboardingAnswers, QuestionLogEntry, ReviewTimeConfig, SummarizerConfig,
    TransportConfig,
};
use super::scan::{CollectorStatus, EvidenceKind, ScanReport};

/// Render the hardcoded baseline `OnboardingAnswers` from the scan
/// report. The reasoning is encoded as a [`QuestionLogEntry`] per
/// question so the audit log is non-empty even on the fallback path
/// (the user can see *why* each default was picked without an LLM in
/// the loop).
pub fn baseline_answers(scan: &ScanReport) -> OnboardingAnswers {
    // Build the per-collector enable map from the scan. We only turn
    // on a collector when the scan reports `Available` (or
    // `AlreadyConfigured`, which the user has already opted into).
    let github_enabled = has_status(scan, "github", |s| {
        matches!(
            s,
            CollectorStatus::Available | CollectorStatus::AlreadyConfigured
        )
    });
    let claude_sessions_enabled = has_status(scan, "claude_sessions", |s| {
        matches!(
            s,
            CollectorStatus::Available | CollectorStatus::AlreadyConfigured
        )
    });
    let calendar_enabled = has_status(scan, "calendar", |s| {
        matches!(
            s,
            CollectorStatus::Available | CollectorStatus::AlreadyConfigured
        )
    });

    // claude_sessions_paths: pull the absolute paths from the
    // DirExists evidence. The ScanReport's `EvidenceKind` carries the
    // path the probe found; we use that directly so the user doesn't
    // have to retype it.
    let claude_sessions_paths: Vec<String> = if claude_sessions_enabled {
        scan.candidates
            .iter()
            .find(|c| c.collector_id == "claude_sessions")
            .and_then(|c| match &c.evidence {
                EvidenceKind::DirExists { path } => Some(path.display().to_string()),
                _ => None,
            })
            .into_iter()
            .collect()
    } else {
        Vec::new()
    };

    // Question log: one entry per question, with the evidence_refs
    // set to the relevant collector_id when present.
    let question_log = vec![
        q(
            "Which claude_sessions paths should the collector monitor?",
            if claude_sessions_paths.is_empty() {
                "scan returned no ~/.claude/projects/ or ~/.claude/sessions/ directory; defaulting to empty list"
            } else {
                "scan reported a single ~/.claude/projects/ directory; using that as the only monitored path"
            },
            if claude_sessions_enabled { vec!["claude_sessions".to_string()] } else { vec![] },
        ),
        q(
            "Enable the github collector?",
            if github_enabled {
                "scan found ~/.config/gh/hosts.yml (and/or `gh auth status`); enabling the github collector with no specific repo filter"
            } else {
                "scan found no GitHub CLI artifacts; defaulting to disabled"
            },
            if github_enabled { vec!["github".to_string()] } else { vec![] },
        ),
        q(
            "Enable the calendar collector?",
            if calendar_enabled {
                "scan found a macOS Calendar.app bundle / Linux evolution store; enabling the calendar collector"
            } else {
                "scan found no calendar artifacts; defaulting to disabled"
            },
            if calendar_enabled { vec!["calendar".to_string()] } else { vec![] },
        ),
        q(
            "Enable voice capture?",
            "voice is GPU-bound and the baseline is conservative; defaulting to disabled (user can opt in later via Settings)",
            vec![],
        ),
        q(
            "What cadence for the daily review?",
            "evening at 18:00 (the wizard shows 18:00 in the user's local timezone and stores the equivalent UTC hour; the LLM's UTC hour is overridden client-side)",
            vec![],
        ),
        q(
            "Use local ollama for the summarizer?",
            "ollama may not be installed; baseline picks the stub backend to keep onboarding working offline",
            vec![],
        ),
        q(
            "Tailscale or ssh for the VPS transport?",
            "ssh uses the keychain-stored ed25519 key from item 1-2; tailscale is preferred when MagicDNS resolves (no key exchange)",
            vec![],
        ),
    ];

    OnboardingAnswers {
        claude_sessions_paths,
        github: if github_enabled {
            Some(GitHubConfig {
                enabled: true,
                repos: Vec::new(),
                include_private: false,
            })
        } else {
            None
        },
        calendar_ics: if calendar_enabled {
            Some(super::answers::CalendarConfig {
                enabled: true,
                ics_paths: Vec::new(),
                calendar_app_id: None,
            })
        } else {
            None
        },
        voice: None,
        review_time: ReviewTimeConfig {
            cadence: "evening".to_string(),
            hour_utc: 18,
        },
        summarizer: SummarizerConfig {
            backend: "stub".to_string(),
            model: "stub".to_string(),
        },
        transport: TransportConfig {
            method: "ssh".to_string(),
            ssh_key_path: None,
        },
        question_log,
    }
}

/// `true` if the scan has a candidate with `collector_id == id` and
/// whose status satisfies `pred`. Returns `false` when the candidate
/// is missing (defensive — the scan should always report all 8).
fn has_status<F>(scan: &ScanReport, id: &str, pred: F) -> bool
where
    F: Fn(&CollectorStatus) -> bool,
{
    scan.candidates
        .iter()
        .find(|c| c.collector_id == id)
        .map(|c| pred(&c.status))
        .unwrap_or(false)
}

/// Build one [`QuestionLogEntry`]. Convenience wrapper so the
/// `vec![]` above reads naturally.
fn q(question: &str, reasoning: &str, evidence_refs: Vec<String>) -> QuestionLogEntry {
    QuestionLogEntry {
        question: question.to_string(),
        reasoning: reasoning.to_string(),
        evidence_refs,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding::scan::{CollectorCandidate, CollectorStatus, Platform};
    use chrono::Utc;

    /// Build a stub scan with the given `(id, status)` pairs.
    fn scan_with(pairs: &[(&str, CollectorStatus)]) -> ScanReport {
        let candidates = pairs
            .iter()
            .map(|(id, status)| CollectorCandidate {
                collector_id: (*id).to_string(),
                display_name: (*id).to_string(),
                status: *status,
                evidence: match status {
                    CollectorStatus::Available | CollectorStatus::AlreadyConfigured => {
                        EvidenceKind::DirExists {
                            path: std::path::PathBuf::from(format!("/tmp/{id}")),
                        }
                    }
                    CollectorStatus::Unavailable => EvidenceKind::FileExists {
                        path: std::path::PathBuf::new(),
                    },
                },
                confidence: 0.5,
                notes: None,
            })
            .collect();
        ScanReport {
            generated_at: Utc::now(),
            platform: Platform::Linux,
            candidates,
        }
    }

    #[test]
    fn empty_scan_disables_every_optional_collector() {
        let scan = scan_with(&[]);
        let ans = baseline_answers(&scan);
        assert!(ans.claude_sessions_paths.is_empty());
        assert!(ans.github.is_none());
        assert!(ans.calendar_ics.is_none());
        assert!(ans.voice.is_none());
        // Mandatory fields always populate.
        assert_eq!(ans.review_time.cadence, "evening");
        assert_eq!(ans.summarizer.backend, "stub");
        assert_eq!(ans.transport.method, "ssh");
        // The question log is non-empty — the baseline is auditable.
        assert!(!ans.question_log.is_empty());
    }

    #[test]
    fn scan_with_github_available_enables_github_collector() {
        let scan = scan_with(&[("github", CollectorStatus::Available)]);
        let ans = baseline_answers(&scan);
        let gh = ans.github.as_ref().expect("github enabled");
        assert!(gh.enabled);
        assert!(gh.repos.is_empty());
        assert!(!gh.include_private);
    }

    #[test]
    fn scan_with_claude_sessions_dir_populates_path_list() {
        let scan = scan_with(&[("claude_sessions", CollectorStatus::Available)]);
        let ans = baseline_answers(&scan);
        assert_eq!(ans.claude_sessions_paths.len(), 1);
        assert!(ans.claude_sessions_paths[0].ends_with("/claude_sessions"));
    }

    #[test]
    fn scan_with_already_configured_collector_keeps_it_enabled() {
        let scan = scan_with(&[("github", CollectorStatus::AlreadyConfigured)]);
        let ans = baseline_answers(&scan);
        assert!(ans.github.is_some());
    }

    #[test]
    fn every_question_in_log_has_non_empty_question_field() {
        let scan = scan_with(&[]);
        let ans = baseline_answers(&scan);
        for entry in &ans.question_log {
            assert!(!entry.question.is_empty());
            assert!(!entry.reasoning.is_empty());
        }
    }
}
