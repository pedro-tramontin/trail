//! Typed `OnboardingAnswers` + the raw LLM-envelope shape.
//!
//! Two parallel types live here:
//!
//! * [`OnboardingAnswers`] — the flattened, downstream-facing struct that
//!   Phase C (config-writer) consumes. It's a clean typed surface: no
//!   `evidence_refs`, no per-question reasoning — just the chosen
//!   values plus the audit log.
//! * [`OnboardingEnvelope`] — the LLM-response shape that mirrors
//!   `schemas/onboarding-answer.schema.json`. Each top-level field is
//!   wrapped in `{ selected, notes, evidence_refs }` (or, for the
//!   enum-valued fields, `{ selected, <enum field>, notes, evidence_refs }`)
//!   so the model can express "I picked X because the scan reported
//!   collector_id Y, and here's my reasoning".
//!
//! The two are deliberately separate types rather than one `serde(flatten)`'d
//! struct: keeps the audit log + evidence_refs out of the downstream
//! `OnboardingAnswers::to_config` path (item 6-3 will be cleaner for it),
//! and lets `ask_onboarding` deserialise the raw ollama response into
//! [`OnboardingEnvelope`] before validating against the schema and
//! flattening.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Downstream typed surface — the Phase C input.
// ---------------------------------------------------------------------------

/// The full set of choices the LLM (or the baseline fallback) picked
/// for this onboarding run. Phase C (`config-writer`, item 6-3)
/// consumes this struct directly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OnboardingAnswers {
    /// Absolute paths to `~/.claude/projects/*` directories the
    /// claude_sessions collector should monitor. Empty when
    /// claude_sessions is `Unavailable` and the user opted out.
    pub claude_sessions_paths: Vec<String>,
    /// GitHub collector configuration. `None` means "do not enable".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub github: Option<GitHubConfig>,
    /// Calendar collector configuration. `None` means "do not enable".
    ///
    /// On macOS the `calendar_app_id == Some("event_kit")` variant
    /// asks the user to grant Full Calendar Access the first time
    /// the collector runs. The wizard surfaces this requirement
    /// via the EventKit hint in `StepAsk.svelte` (calendar row),
    /// which uses the per-OS deep-link button emitted by
    /// `commands::calendar_permission_deep_link_url` (see
    /// `commands::calendar_permission_deep_link_url_for` for the
    /// per-OS dispatch table). On Linux the helper returns
    /// `Err(CalendarPermissionDeepLinkError::UnknownDE)` and the
    /// wizard renders a labeled "open Settings → Privacy →
    /// Calendar manually" fallback.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_ics: Option<CalendarConfig>,
    /// 2026-08-11 — Browser-history pick list. Each entry is
    /// a browser ID the user enabled on the Ask step
    /// (`"chrome"`, `"brave"`, `"firefox"`, `"opera"`,
    /// `"safari"`). `None` means the field wasn't pre-filled
    /// by the LLM; the Ask step's edit-mode handles the
    /// picker UI. The actual collector that reads these
    /// files is built in a follow-up PR — for now this is
    /// captured but not consumed by Phase C.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser_history: Option<Vec<String>>,
    /// Voice capture configuration. `None` means "do not enable".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<VoiceConfig>,
    /// When the daily review should fire. Always present — the
    /// scheduler is mandatory.
    pub review_time: ReviewTimeConfig,
    /// Which summarizer backend to use. Always present.
    pub summarizer: SummarizerConfig,
    /// Which transport to use for VPS push. Always present.
    pub transport: TransportConfig,
    /// Per-question reasoning trace. Preserved end-to-end so the user
    /// (or an auditor) can later see *why* the LLM picked a default.
    pub question_log: Vec<QuestionLogEntry>,
}

impl Default for OnboardingAnswers {
    /// The "everything off, no log" empty state. Useful for tests
    /// that build up the struct piecewise. The [`baseline_answers`]
    /// fallback returns a more opinionated default; `Default` here
    /// is the literal zero.
    ///
    /// `review_time.hour_utc` defaults to 18. The Svelte wizard
    /// treats this as a local-hour value (18:00 in the user's
    /// timezone) and translates it to the equivalent UTC hour
    /// before sending the answers to `write_onboarding_config`.
    /// The Rust side stays UTC-only; the timezone handling lives
    /// in the wizard so the scheduler (which parses `%H:%M` as
    /// UTC) doesn't need to change. See
    /// `src/lib/onboarding/StepAsk.svelte`'s `apply_local_review_time`
    /// for the conversion. A future item can persist the IANA
    /// timezone string alongside `hour_utc` and let the user
    /// override the local hour in Settings.
    fn default() -> Self {
        Self {
            claude_sessions_paths: Vec::new(),
            github: None,
            calendar_ics: None,
            browser_history: None,
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
            question_log: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitHubConfig {
    pub enabled: bool,
    /// `owner/repo` slugs the collector should watch. Empty when
    /// the user has no orgs / wants all-public.
    #[serde(default)]
    pub repos: Vec<String>,
    /// `true` includes private repos the user can see via `gh auth`.
    /// Default `false` — the LLM is told to opt-in conservatively.
    #[serde(default)]
    pub include_private: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CalendarConfig {
    pub enabled: bool,
    /// Absolute paths to `.ics` files (or, on macOS, the Calendar.app
    /// saved-state dir paths). Empty when the user defers to Calendar.app's
    /// default calendars.
    #[serde(default)]
    pub ics_paths: Vec<String>,
    /// Optional macOS Calendar.app bundle id (e.g. `"com.apple.iCal"`)
    /// when the user wants to use the system calendar app rather than
    /// raw .ics files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_app_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceConfig {
    pub enabled: bool,
    /// `"tiny" | "base" | "small"` — whisper model size. Default `"base"`.
    pub model: String,
    /// `"en" | "pt"` — BCP-47 primary subtag. Default `"en"`.
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReviewTimeConfig {
    /// `"morning" | "evening" | "weekly"`. Maps to a coarse hour-of-day
    /// bucket; the LLM can override `hour_utc` if the scan's platform
    /// + locale suggests a different local-time equivalent.
    pub cadence: String,
    /// Hour-of-day in UTC, 0-23 inclusive.
    pub hour_utc: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SummarizerConfig {
    /// `"ollama" | "stub"`. `"stub"` is a no-op pass-through that
    /// returns a templated "no summary available" string; useful for
    /// laptops without a working ollama install.
    pub backend: String,
    /// Ollama model name (e.g. `"qwen2.5:7b"`) when `backend == "ollama"`,
    /// or the literal string `"stub"` when `backend == "stub"`.
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransportConfig {
    /// `"tailscale" | "ssh"`. `"tailscale"` uses the MagicDNS hostname;
    /// `"ssh"` uses the OS-credential-store-stored ed25519 key from
    /// item 1-2 (macOS Keychain / Linux secret-service / Windows
    /// Credential Manager — see `credential_store_name()`).
    pub method: String,
    /// Filesystem path to the SSH private key (only meaningful for
    /// `method == "ssh"`; the prod path uses the OS credential store
    /// (macOS Keychain / Linux secret-service / Windows Credential
    /// Manager) and leaves this `None`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh_key_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuestionLogEntry {
    /// The literal question that was asked of the model (verbatim, for
    /// auditability).
    pub question: String,
    /// One-paragraph free-form reasoning. The model's own words; an
    /// auditor reads this when the user later disputes a default.
    pub reasoning: String,
    /// ScanReport `collector_id`s the model referenced when answering
    /// this question. Empty when the answer was a no-evidence default.
    #[serde(default)]
    pub evidence_refs: Vec<String>,
}

// ---------------------------------------------------------------------------
// LLM-envelope surface — what ollama actually returns.
// ---------------------------------------------------------------------------

/// Raw shape of an ollama `/api/chat` response when the model is
/// configured to emit JSON. The `message.content` field carries the
/// JSON string we'll deserialise into [`OnboardingEnvelope`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaChatResponse {
    pub model: String,
    pub message: OllamaChatMessage,
    #[serde(default)]
    pub done: bool,
    #[serde(default)]
    pub done_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaChatMessage {
    pub role: String,
    pub content: String,
}

/// The schema-validated envelope. Each field mirrors
/// `schemas/onboarding-answer.schema.json`; the `ask_onboarding`
/// consumer validates this against the schema, then flattens it via
/// [`OnboardingEnvelope::into_typed`] into the downstream
/// [`OnboardingAnswers`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OnboardingEnvelope {
    pub claude_sessions_paths: AnswerFieldBool,
    pub github: AnswerFieldBool,
    pub calendar_ics: AnswerFieldBool,
    /// 2026-08-11 — Browser-history pick list. Same
    /// `AnswerFieldBool` shape as the other data-source
    /// rows so the Ask step's per-row tooltip reasoning
    /// keeps working. `selected = true` means at least
    /// one browser is enabled; the per-browser IDs live
    /// in `notes` (comma-separated, matching the
    /// calendar_ics notes convention).
    pub browser_history: AnswerFieldBool,
    pub voice: AnswerFieldVoice,
    pub review_time: AnswerFieldReviewTime,
    pub summarizer: AnswerFieldSummarizer,
    pub transport: AnswerFieldTransport,
    pub question_log: Vec<QuestionLogEntry>,
}

/// A boolean answer: `selected` + optional notes + optional evidence.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerFieldBool {
    pub selected: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerFieldVoice {
    pub selected: bool,
    pub model: String,
    pub language: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerFieldReviewTime {
    pub selected: bool,
    pub cadence: String,
    pub hour_utc: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerFieldSummarizer {
    pub selected: bool,
    pub backend: String,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AnswerFieldTransport {
    pub selected: bool,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

impl OnboardingEnvelope {
    /// Flatten the schema-validated envelope into the downstream
    /// [`OnboardingAnswers`]. The optional collectors (github /
    /// calendar_ics / voice) are `None` when `selected == false`; the
    /// mandatory ones (review_time / summarizer / transport) always
    /// populate. `claude_sessions_paths` is the *string list* derived
    /// from the envelope's `notes` (one path per line) — the model
    /// encodes the path list inside `notes` because the schema's
    /// `claude_sessions_paths` is the boolean envelope, not the
    /// downstream typed `Vec<String>`. This split keeps the schema
    /// uniform across the eight required fields while letting the
    /// downstream `OnboardingAnswers` stay strongly-typed.
    pub fn into_typed(self) -> OnboardingAnswers {
        let claude_sessions_paths = self
            .claude_sessions_paths
            .notes
            .as_deref()
            .map(|n| {
                n.lines()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();

        let github = if self.github.selected {
            let repos = self
                .github
                .notes
                .as_deref()
                .map(|n| {
                    n.lines()
                        .flat_map(|line| line.split(','))
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Some(GitHubConfig {
                enabled: true,
                repos,
                include_private: false,
            })
        } else {
            None
        };

        let calendar_ics = if self.calendar_ics.selected {
            let ics_paths = self
                .calendar_ics
                .notes
                .as_deref()
                .map(|n| {
                    n.lines()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            Some(CalendarConfig {
                enabled: true,
                ics_paths,
                calendar_app_id: None,
            })
        } else {
            None
        };

        // 2026-08-11 — browser-history. The envelope's
        // `notes` carries a comma-separated list of browser
        // IDs (`chrome`, `brave`, `firefox`, `opera`,
        // `safari`). When `selected = true` we parse it
        // into a `Vec<String>`; when `selected = false` we
        // emit `None` so the Ask step's "disabled" UI
        // renders. The list is trimmed + empties dropped,
        // matching the github row's `repos` parser.
        let browser_history = if self.browser_history.selected {
            self.browser_history.notes.as_deref().map(|n| {
                n.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect()
            })
        } else {
            None
        };

        let voice = if self.voice.selected {
            Some(VoiceConfig {
                enabled: true,
                model: self.voice.model,
                language: self.voice.language,
            })
        } else {
            None
        };

        OnboardingAnswers {
            claude_sessions_paths,
            github,
            calendar_ics,
            browser_history,
            voice,
            review_time: ReviewTimeConfig {
                cadence: self.review_time.cadence,
                hour_utc: self.review_time.hour_utc,
            },
            summarizer: SummarizerConfig {
                backend: self.summarizer.backend,
                model: self.summarizer.model,
            },
            transport: TransportConfig {
                method: self.transport.method,
                ssh_key_path: None,
            },
            question_log: self.question_log,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_with_all_selected_flattens_to_typed_answers() {
        let env = OnboardingEnvelope {
            claude_sessions_paths: AnswerFieldBool {
                selected: true,
                notes: Some("/home/u/.claude/projects/a\n/home/u/.claude/projects/b".to_string()),
                evidence_refs: vec!["claude_sessions".to_string()],
            },
            github: AnswerFieldBool {
                selected: true,
                notes: Some("acme/api, acme/web".to_string()),
                evidence_refs: vec!["github".to_string()],
            },
            calendar_ics: AnswerFieldBool {
                selected: false,
                notes: None,
                evidence_refs: vec![],
            },
            // 2026-08-11 — browser-history. Both test
            // fixtures (all_selected / all_deselected)
            // carry `selected = false` since the typed
            // `OnboardingAnswers.browser_history` is
            // tested independently in the all_selected
            // arm above.
            browser_history: AnswerFieldBool {
                selected: true,
                notes: Some("chrome".to_string()),
                evidence_refs: vec!["chrome_history".to_string()],
            },
            voice: AnswerFieldVoice {
                selected: true,
                model: "base".to_string(),
                language: "en".to_string(),
                notes: None,
                evidence_refs: vec![],
            },
            review_time: AnswerFieldReviewTime {
                selected: true,
                cadence: "evening".to_string(),
                hour_utc: 18,
                notes: None,
                evidence_refs: vec![],
            },
            summarizer: AnswerFieldSummarizer {
                selected: true,
                backend: "ollama".to_string(),
                model: "qwen2.5:7b".to_string(),
                notes: None,
                evidence_refs: vec![],
            },
            transport: AnswerFieldTransport {
                selected: true,
                method: "tailscale".to_string(),
                notes: None,
                evidence_refs: vec![],
            },
            question_log: vec![],
        };
        let typed = env.into_typed();
        assert_eq!(typed.claude_sessions_paths.len(), 2);
        assert!(typed.github.is_some());
        assert_eq!(
            typed.github.as_ref().unwrap().repos,
            vec!["acme/api", "acme/web"]
        );
        assert!(typed.calendar_ics.is_none());
        assert!(typed.voice.is_some());
        assert_eq!(typed.summarizer.backend, "ollama");
        assert_eq!(typed.transport.method, "tailscale");
    }

    #[test]
    fn envelope_with_all_deselected_flattens_to_minimal_typed_answers() {
        let env = OnboardingEnvelope {
            claude_sessions_paths: AnswerFieldBool {
                selected: false,
                notes: None,
                evidence_refs: vec![],
            },
            github: AnswerFieldBool {
                selected: false,
                notes: None,
                evidence_refs: vec![],
            },
            calendar_ics: AnswerFieldBool {
                selected: false,
                notes: None,
                evidence_refs: vec![],
            },
            // 2026-08-11 — browser-history. Both test
            // fixtures (all_selected / all_deselected)
            // carry `selected = false` since the typed
            // `OnboardingAnswers.browser_history` is
            // tested independently in the all_selected
            // arm above.
            browser_history: AnswerFieldBool {
                selected: true,
                notes: Some("chrome".to_string()),
                evidence_refs: vec!["chrome_history".to_string()],
            },
            voice: AnswerFieldVoice {
                selected: false,
                model: "base".to_string(),
                language: "en".to_string(),
                notes: None,
                evidence_refs: vec![],
            },
            review_time: AnswerFieldReviewTime {
                selected: false,
                cadence: "evening".to_string(),
                hour_utc: 18,
                notes: None,
                evidence_refs: vec![],
            },
            summarizer: AnswerFieldSummarizer {
                selected: false,
                backend: "stub".to_string(),
                model: "stub".to_string(),
                notes: None,
                evidence_refs: vec![],
            },
            transport: AnswerFieldTransport {
                selected: false,
                method: "ssh".to_string(),
                notes: None,
                evidence_refs: vec![],
            },
            question_log: vec![],
        };
        let typed = env.into_typed();
        assert!(typed.claude_sessions_paths.is_empty());
        assert!(typed.github.is_none());
        assert!(typed.calendar_ics.is_none());
        assert!(typed.voice.is_none());
        // The mandatory fields always populate regardless of `selected`.
        assert_eq!(typed.review_time.cadence, "evening");
        assert_eq!(typed.summarizer.backend, "stub");
        assert_eq!(typed.transport.method, "ssh");
    }
}
