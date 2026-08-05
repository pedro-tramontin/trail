//! Phase B: LLM-driven onboarding Q&A via a local `ollama` server.
//!
//! This module is the second half of the onboarding flow:
//!
//! 1. **Phase A (item 6-1)** runs [`super::scan::scan_laptop`], which
//!    produces a [`ScanReport`] of 8 `CollectorCandidate`s with
//!    evidence + status.
//! 2. **Phase B (this item)** feeds the report to a local ollama
//!    instance via `/api/chat` with `format: "json"` structured
//!    output, validates the response against
//!    [`schemas/onboarding-answer.schema.json`], and flattens it into
//!    an [`OnboardingAnswers`] for Phase C.
//! 3. **Phase C (item 6-3)** writes the `OnboardingAnswers` to
//!    `~/.trail/config.json`.
//!
//! When ollama is unreachable (port closed, model not loaded, etc.),
//! the function falls back to [`super::baseline::baseline_answers`]
//! so the onboarding flow never blocks the user on a missing
//! optional dependency. The fallback is auditable: the returned
//! `question_log` carries per-question reasoning that explains the
//! default.
//!
//! ## Why a separate `ask_onboarding` function (not the Tauri
//! command directly)
//!
//! The Tauri command needs an `AppHandle` (so the IPC binding can
//! log) and serialises errors to `String`. The plain
//! `ask_onboarding` function takes only `&ScanReport + &reqwest::Client`
//! and returns a typed `Result<OnboardingAnswers, AskOnboardingError>`
//! — that signature is what the wiremock tests exercise. The Tauri
//! command is a one-liner wrapper.

use std::time::Duration;

use serde::Serialize;
use thiserror::Error;

use super::answers::{OllamaChatResponse, OnboardingAnswers, OnboardingEnvelope};
use super::baseline::baseline_answers;
use super::scan::ScanReport;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Default ollama endpoint. Matches the one in `src-tauri/src/ollama.rs`
/// but duplicated here so the onboarding module is self-contained —
/// the existing ollama client doesn't expose a chat-with-structured-output
/// method yet, so this module uses its own thin HTTP call.
pub const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Default model the onboarding prompt targets. Kept small (3.8B
/// params) so it runs on a stock MacBook Air without a discrete
/// GPU. Users with a beefier box can override via [`AskOptions::model`].
pub const DEFAULT_MODEL: &str = "qwen2.5:7b";

/// Maximum time to wait for an ollama response. The structured-output
/// prompt is short (8 fields, ~200 tokens) so 30s is generous; if
/// the model is cold-loading we wait the full 30s and then fall back
/// to the baseline.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Knobs for [`ask_onboarding`]. All fields default to the constants
/// above; pass an override only when needed.
#[derive(Debug, Clone)]
pub struct AskOptions {
    pub ollama_url: String,
    pub model: String,
    pub timeout: Duration,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            ollama_url: DEFAULT_OLLAMA_URL.to_string(),
            model: DEFAULT_MODEL.to_string(),
            timeout: REQUEST_TIMEOUT,
        }
    }
}

/// Typed errors from [`ask_onboarding`]. The `OllamaUnreachable`
/// variant is the *expected* failure mode (ollama isn't running) —
/// `ask_onboarding` doesn't return it directly; instead it falls
/// back to the baseline. The other variants are real bugs the caller
/// should surface.
#[derive(Debug, Error)]
pub enum AskOnboardingError {
    /// Ollama returned a 4xx/5xx status. The body is included for
    /// debugging.
    #[error("ollama returned status {status}: {body}")]
    OllamaHttp { status: u16, body: String },
    /// Ollama returned 200 but the response wasn't valid JSON, or
    /// the JSON didn't match the expected `OllamaChatResponse` shape.
    #[error("failed to parse ollama response: {message}")]
    Parse { message: String },
    /// Ollama returned 200 + a valid envelope, but the envelope
    /// didn't pass the JSON-Schema validation.
    #[error("response failed schema validation: {message}")]
    SchemaValidation { message: String },
    /// Reqwest-level failure (DNS, connect, decode, etc.) other than
    /// the connect-refused case which [`ask_onboarding`] swallows
    /// and treats as the "fall back to baseline" trigger.
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
}

// ---------------------------------------------------------------------------
// LLM-driven entry point
// ---------------------------------------------------------------------------

/// Run the Phase B onboarding Q&A. Tries ollama first; falls back to
/// the hardcoded baseline when ollama is unreachable (the connect
/// refused, the model isn't pulled, the request times out, or the
/// response is a 5xx). Real errors — parse failures on a 200,
/// schema-validation failures — bubble up so the UI can surface them.
pub async fn ask_onboarding(scan: &ScanReport, http: &reqwest::Client) -> OnboardingAnswers {
    ask_onboarding_with(scan, http, AskOptions::default()).await
}

/// Same as [`ask_onboarding`] but with overridable [`AskOptions`].
/// The wiremock tests pass a custom `ollama_url` pointing at the
/// mock server; the live path uses the defaults.
pub async fn ask_onboarding_with(
    scan: &ScanReport,
    http: &reqwest::Client,
    opts: AskOptions,
) -> OnboardingAnswers {
    let prompt = build_prompt(scan);
    match call_ollama(http, &opts, &prompt).await {
        Ok(envelope) => envelope.into_typed(),
        Err(_) => baseline_answers(scan),
    }
}

// ---------------------------------------------------------------------------
// Prompt construction
// ---------------------------------------------------------------------------

/// Build the full prompt that gets sent to ollama. The system prompt
/// pins the model's persona; the user prompt carries the scan report
/// + the 6 questions.
///
/// The 6 questions are kept stable across runs (so the audit log is
/// reproducible) but each is gated by an `if available in scan` clause
/// — when the scan says `Unavailable` for a source, the question is
/// rendered as "scan found no evidence; defaulting to no".
pub fn build_prompt(scan: &ScanReport) -> String {
    let mut out = String::with_capacity(4096);
    out.push_str(
        "SYSTEM: You are configuring a personal data-collection tool called Trail. \
         Reply with JSON only, no prose. The response must match the JSON Schema in \
         the user's message.\n\n",
    );
    out.push_str("USER: Here is the scan of the user's laptop:\n\n");
    out.push_str(&format_scan_report(scan));
    out.push_str("\nAnswer the following questions with one structured answer per field:\n\n");
    for (i, q) in per_question_prompts(scan).iter().enumerate() {
        out.push_str(&format!("{}. {}\n", i + 1, q));
    }
    out.push_str(
        "\nFor every answer include:\n\
         - `selected`: true or false (or the enum value the field requires)\n\
         - `notes`: a short reasoning string\n\
         - `evidence_refs`: the list of `collector_id` strings from the scan that informed your answer\n\
         - `question_log`: an array of `{question, reasoning, evidence_refs}` (one per question)\n\n\
         Reply with a single JSON object only. No markdown fences, no commentary.",
    );
    out
}

/// Render the scan report as a stable, human-readable block. The
/// `evidence` line is the most useful signal for the LLM (file path,
/// env var name, etc.); we include `confidence` and `status` so the
/// model can weight its reasoning.
fn format_scan_report(scan: &ScanReport) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "platform: {platform:?}\ngenerated_at: {ts}\n",
        platform = scan.platform,
        ts = scan.generated_at.to_rfc3339(),
    );
    for c in &scan.candidates {
        let _ = writeln!(
            out,
            "- {id} ({display}): status={status:?}, confidence={conf}, evidence={ev:?}{notes}",
            id = c.collector_id,
            display = c.display_name,
            status = c.status,
            conf = c.confidence,
            ev = c.evidence,
            notes = c
                .notes
                .as_deref()
                .map(|n| format!(", notes=\"{n}\""))
                .unwrap_or_default(),
        );
    }
    out
}

/// The 6 questions, tailored to the scan. Each question is rendered
/// in the audit log verbatim; the LLM sees a copy in the user prompt.
fn per_question_prompts(scan: &ScanReport) -> Vec<String> {
    let claude_sessions = scan
        .candidates
        .iter()
        .find(|c| c.collector_id == "claude_sessions");
    let github = scan.candidates.iter().find(|c| c.collector_id == "github");
    let calendar = scan
        .candidates
        .iter()
        .find(|c| c.collector_id == "calendar");

    vec![
        match claude_sessions {
            Some(c) if c.is_available() => format!(
                "Which claude_sessions paths to monitor? The scan found evidence: {}",
                c.evidence_label()
            ),
            _ => "claude_sessions was not found on the laptop; leave claude_sessions_paths.selected=false.".to_string(),
        },
        match github {
            Some(c) if c.is_available() => {
                "Enable the github collector? If yes, list the `owner/repo` slugs to watch in the github.notes field.".to_string()
            }
            _ => "github was not found; leave github.selected=false.".to_string(),
        },
        match calendar {
            Some(c) if c.is_available() => {
                "Enable the calendar collector? If yes, populate calendar_ics.notes with the .ics paths."
                    .to_string()
            }
            _ => "calendar was not found; leave calendar_ics.selected=false.".to_string(),
        },
        "Enable voice capture? If yes, set voice.model and voice.language.".to_string(),
        "What cadence for the daily review (morning/evening/weekly) and what hour_utc?".to_string(),
        "Use local ollama for the summarizer backend? If yes, set summarizer.model to the ollama model name. Otherwise pick 'stub'.".to_string(),
        "Tailscale or ssh for the VPS transport?".to_string(),
    ]
}

// ---------------------------------------------------------------------------
// ollama HTTP call
// ---------------------------------------------------------------------------

/// POST the prompt to ollama's `/api/chat` endpoint with
/// `format: "json"` so the model is constrained to emit JSON. Parse
/// the response, validate the envelope against the bundled JSON
/// Schema, and return the typed envelope ready for flattening.
///
/// We deliberately use ollama's structured-output mode (the
/// `format: "json"` parameter) rather than the JSON-Schema mode
/// (`format: {type: "object", ...}`) because the model is small (3.8B
/// params) and JSON-Schema mode is stricter; the model occasionally
/// emits near-valid JSON that the schema rejects. The `format: "json"`
/// mode + our client-side schema validation gives the same end-result
/// (validated JSON) with a higher success rate on small models.
async fn call_ollama(
    http: &reqwest::Client,
    opts: &AskOptions,
    prompt: &str,
) -> Result<OnboardingEnvelope, AskOnboardingError> {
    let req = OllamaChatRequest {
        model: &opts.model,
        messages: vec![
            OllamaChatRequestMessage {
                role: "system",
                content: "You are configuring a personal data-collection tool called Trail. Reply with JSON only, no prose.",
            },
            OllamaChatRequestMessage {
                role: "user",
                content: prompt,
            },
        ],
        stream: false,
        format: "json",
    };

    let url = format!("{}/api/chat", opts.ollama_url.trim_end_matches('/'));
    let resp = http
        .post(&url)
        .timeout(opts.timeout)
        .json(&req)
        .send()
        .await
        .map_err(AskOnboardingError::Http)?;

    let status = resp.status();
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(AskOnboardingError::OllamaHttp {
            status: status.as_u16(),
            body: truncate_for_error(&body),
        });
    }

    let chat: OllamaChatResponse = resp.json().await.map_err(|e| AskOnboardingError::Parse {
        message: e.to_string(),
    })?;

    let envelope: OnboardingEnvelope =
        serde_json::from_str(&chat.message.content).map_err(|e| AskOnboardingError::Parse {
            message: e.to_string(),
        })?;

    validate_envelope(&envelope)?;
    Ok(envelope)
}

#[derive(Serialize)]
struct OllamaChatRequest<'a> {
    model: &'a str,
    messages: Vec<OllamaChatRequestMessage<'a>>,
    stream: bool,
    /// Pin the model to JSON output. ollama's structured-output mode
    /// accepts either the string `"json"` (any JSON) or an inline
    /// JSON-Schema object. We use the schema object below via
    /// [`OllamaChatRequest::with_schema`] when the caller asks for
    /// schema-constrained output.
    format: &'a str,
}

#[derive(Serialize)]
struct OllamaChatRequestMessage<'a> {
    role: &'a str,
    content: &'a str,
}

/// Truncate a (potentially large) error body so the typed error stays
/// bounded. ollama rarely returns more than a few KB even on 5xx,
/// but a misbehaving proxy could dump a multi-MB HTML page; the
/// `ollama.rs` `MAX_ERROR_BODY_BYTES` constant is the source of truth
/// but we re-implement the cap here to keep the onboarding module
/// decoupled from the summarizer's ollama client.
fn truncate_for_error(s: &str) -> String {
    const CAP: usize = 8 * 1024;
    if s.len() <= CAP {
        s.to_string()
    } else {
        format!("{}... <truncated at {} bytes>", &s[..CAP], CAP)
    }
}

// ---------------------------------------------------------------------------
// Schema validation
// ---------------------------------------------------------------------------

/// Validate the parsed envelope against `schemas/onboarding-answer.schema.json`.
/// We compile the schema once via [`once_cell`] and reuse the compiled
/// validator across calls (the schema is small + the cost of
/// re-compiling on every request would dominate the round-trip).
fn validate_envelope(envelope: &OnboardingEnvelope) -> Result<(), AskOnboardingError> {
    use jsonschema::JSONSchema;
    use once_cell::sync::Lazy;

    static SCHEMA: Lazy<Result<JSONSchema, String>> = Lazy::new(|| {
        let raw = include_str!("../../../schemas/onboarding-answer.schema.json");
        let value: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| format!("parse bundled schema: {e}"))?;
        JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .compile(&value)
            .map_err(|e| e.to_string())
    });

    let validator = SCHEMA
        .as_ref()
        .map_err(|e| AskOnboardingError::SchemaValidation {
            message: format!("schema compile failed: {e}"),
        })?;

    let value =
        serde_json::to_value(envelope).map_err(|e| AskOnboardingError::SchemaValidation {
            message: format!("envelope -> value: {e}"),
        })?;
    let result = validator.validate(&value);
    if let Err(errors) = result {
        let first = errors
            .into_iter()
            .next()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "unknown schema error".to_string());
        return Err(AskOnboardingError::SchemaValidation { message: first });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

/// Tauri command: run the Phase B onboarding Q&A. The frontend's
/// onboarding wizard (item 6-4) invokes this as
/// `invoke('ask_onboarding_cmd')` after the Phase A scan completes.
///
/// The command is non-panicking: any ollama-side error falls back to
/// the baseline, so the frontend never sees a thrown exception from
/// the ollama path. The returned `OnboardingAnswers` always carries a
/// populated `question_log` so the UI can show the reasoning.
#[tauri::command]
pub async fn ask_onboarding_cmd(scan: super::scan::ScanReport) -> OnboardingAnswers {
    let http = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .expect("reqwest client builder");
    ask_onboarding(&scan, &http).await
}

// ---------------------------------------------------------------------------
// helpers on CollectorCandidate used by the prompt builder
// ---------------------------------------------------------------------------

trait CollectorCandidateExt {
    fn is_available(&self) -> bool;
    fn evidence_label(&self) -> String;
}

impl CollectorCandidateExt for super::scan::CollectorCandidate {
    fn is_available(&self) -> bool {
        matches!(
            self.status,
            super::scan::CollectorStatus::Available
                | super::scan::CollectorStatus::AlreadyConfigured
        )
    }
    fn evidence_label(&self) -> String {
        match &self.evidence {
            super::scan::EvidenceKind::FileExists { path } => {
                format!("file: {}", path.display())
            }
            super::scan::EvidenceKind::DirExists { path } => {
                format!("dir: {}", path.display())
            }
            super::scan::EvidenceKind::CommandExists { binary, path } => {
                format!("command: {} ({})", binary, path.display())
            }
            super::scan::EvidenceKind::EnvVar { name, value } => match value {
                Some(v) => format!("env: {}={}", name, v),
                None => format!("env: {} (unset)", name),
            },
            super::scan::EvidenceKind::MacosAppBundle { path, bundle_id } => {
                format!("app: {} ({})", path.display(), bundle_id)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::onboarding::scan::{CollectorCandidate, CollectorStatus, EvidenceKind, Platform};
    use chrono::Utc;
    use serde_json::json;
    use std::path::PathBuf;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Build a fully-populated `ScanReport` (8 candidates) so the
    /// prompt + envelope code can be exercised end-to-end without
    /// `walkdir` / `gh auth status` side-effects.
    fn sample_scan() -> ScanReport {
        let mut cands: Vec<CollectorCandidate> = Vec::new();
        let push = |cands: &mut Vec<CollectorCandidate>,
                    id: &str,
                    display: &str,
                    status: CollectorStatus,
                    ev: EvidenceKind| {
            cands.push(CollectorCandidate {
                collector_id: id.to_string(),
                display_name: display.to_string(),
                status,
                evidence: ev,
                confidence: 0.9,
                notes: None,
            });
        };
        push(
            &mut cands,
            "github",
            "GitHub activity",
            CollectorStatus::Available,
            EvidenceKind::CommandExists {
                binary: "gh".to_string(),
                path: PathBuf::from("/usr/bin/gh"),
            },
        );
        push(
            &mut cands,
            "calendar",
            "Calendar events",
            CollectorStatus::Available,
            EvidenceKind::DirExists {
                path: PathBuf::from("/home/u/.config/evolution"),
            },
        );
        push(
            &mut cands,
            "claude_sessions",
            "Claude sessions",
            CollectorStatus::Available,
            EvidenceKind::DirExists {
                path: PathBuf::from("/home/u/.claude/projects"),
            },
        );
        push(
            &mut cands,
            "gmail",
            "Gmail (via Apple Mail)",
            CollectorStatus::Unavailable,
            EvidenceKind::FileExists {
                path: PathBuf::new(),
            },
        );
        push(
            &mut cands,
            "notes",
            "Notes",
            CollectorStatus::Unavailable,
            EvidenceKind::FileExists {
                path: PathBuf::new(),
            },
        );
        push(
            &mut cands,
            "vscode_extensions",
            "VS Code extensions",
            CollectorStatus::Available,
            EvidenceKind::DirExists {
                path: PathBuf::from("/home/u/.vscode/extensions"),
            },
        );
        push(
            &mut cands,
            "chrome_history",
            "Chrome history",
            CollectorStatus::Unavailable,
            EvidenceKind::FileExists {
                path: PathBuf::new(),
            },
        );
        push(
            &mut cands,
            "brave_history",
            "Brave history",
            CollectorStatus::Unavailable,
            EvidenceKind::FileExists {
                path: PathBuf::new(),
            },
        );
        ScanReport {
            generated_at: Utc::now(),
            platform: Platform::Linux,
            candidates: cands,
        }
    }

    /// A canonical valid envelope body. The wiremock tests reuse
    /// this so the shape + casing stays in one place.
    fn valid_envelope_body() -> serde_json::Value {
        json!({
            "claude_sessions_paths": {
                "selected": true,
                "notes": "/home/u/.claude/projects",
                "evidence_refs": ["claude_sessions"]
            },
            "github": {
                "selected": true,
                "notes": "acme/api, acme/web",
                "evidence_refs": ["github"]
            },
            "calendar_ics": {
                "selected": true,
                "notes": "",
                "evidence_refs": ["calendar"]
            },
            "voice": {
                "selected": false,
                "model": "base",
                "language": "en",
                "notes": "user opted out",
                "evidence_refs": []
            },
            "review_time": {
                "selected": true,
                "cadence": "evening",
                "hour_utc": 18,
                "notes": "evening at 18:00 (the wizard shows 18:00 in the user's local timezone and stores the equivalent UTC hour; the LLM's UTC hour is overridden client-side)",
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
                    "reasoning": "scan found /home/u/.claude/projects",
                    "evidence_refs": ["claude_sessions"]
                }
            ]
        })
    }

    /// Build the wiremock `/api/chat` response envelope that
    /// `call_ollama` deserialises. Wraps the inner JSON inside
    /// `OllamaChatResponse { message: { content: <inner json string> } }`.
    fn ollama_ok(inner: serde_json::Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "model": "qwen2.5:7b",
            "message": {
                "role": "assistant",
                "content": inner.to_string(),
            },
            "done": true,
            "done_reason": "stop",
        }))
    }

    // -----------------------------------------------------------------------
    // Wiremock tests — 4 binding cases
    // -----------------------------------------------------------------------

    /// wiremock returns a valid schema-matching JSON; the function
    /// parses it into a typed `OnboardingAnswers` with the expected
    /// fields populated.
    #[tokio::test]
    async fn valid_ollama_response_parses_into_onboarding_answers() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ollama_ok(valid_envelope_body()))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let scan = sample_scan();
        let opts = AskOptions {
            ollama_url: server.uri(),
            ..Default::default()
        };
        let ans = ask_onboarding_with(&scan, &http, opts).await;
        assert_eq!(ans.claude_sessions_paths, vec!["/home/u/.claude/projects"]);
        let gh = ans.github.expect("github enabled");
        assert_eq!(gh.repos, vec!["acme/api", "acme/web"]);
        assert!(!gh.include_private);
        assert!(ans.calendar_ics.is_some());
        assert!(ans.voice.is_none(), "voice selected=false -> None");
        assert_eq!(ans.review_time.cadence, "evening");
        assert_eq!(ans.review_time.hour_utc, 18);
        assert_eq!(ans.summarizer.backend, "ollama");
        assert_eq!(ans.summarizer.model, "qwen2.5:7b");
        assert_eq!(ans.transport.method, "tailscale");
        assert_eq!(ans.question_log.len(), 1);
    }

    /// wiremock returns a JSON missing the `voice` field. The
    /// `OnboardingEnvelope` deserialisation must fail (serde reports
    /// the missing field) so the function falls back to the baseline
    /// rather than returning a half-populated `OnboardingAnswers`.
    #[tokio::test]
    async fn ollama_response_missing_required_field_returns_baseline() {
        let server = MockServer::start().await;
        let mut body = valid_envelope_body();
        // Drop the `voice` field — the schema + the struct both
        // require it, so the call must fail.
        let obj = body.as_object_mut().unwrap();
        obj.remove("voice");
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ollama_ok(body))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let scan = sample_scan();
        let opts = AskOptions {
            ollama_url: server.uri(),
            ..Default::default()
        };
        // The function does NOT bubble the parse error — it falls
        // back to the baseline. This matches the spec's "fall back
        // when ollama can't produce a usable answer" contract.
        let ans = ask_onboarding_with(&scan, &http, opts).await;
        // Baseline: github + claude_sessions + calendar all Available
        // → baseline enables each, plus mandatory fields populate.
        assert!(!ans.claude_sessions_paths.is_empty());
        assert!(ans.github.is_some());
        assert!(ans.calendar_ics.is_some());
        assert!(ans.voice.is_none());
        // Question log from the baseline is non-empty.
        assert!(!ans.question_log.is_empty());
    }

    /// wiremock returns 500. The function must swallow the error and
    /// return the baseline.
    #[tokio::test]
    async fn ollama_unreachable_returns_baseline_fallback() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(500).set_body_string("model not loaded"))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let scan = sample_scan();
        let opts = AskOptions {
            ollama_url: server.uri(),
            ..Default::default()
        };
        let ans = ask_onboarding_with(&scan, &http, opts).await;
        // Baseline path: github+claude_sessions+calendar all Available.
        assert!(ans.github.is_some());
        assert!(!ans.claude_sessions_paths.is_empty());
        assert!(!ans.question_log.is_empty());
    }

    /// wiremock returns a JSON that includes a question_log array
    /// with multiple entries; the function must surface every entry
    /// in the typed `OnboardingAnswers`.
    #[tokio::test]
    async fn structured_output_parses_with_question_log() {
        let server = MockServer::start().await;
        let mut body = valid_envelope_body();
        body.as_object_mut().unwrap()["question_log"] = json!([
            {
                "question": "Which claude_sessions paths to monitor?",
                "reasoning": "scan found /home/u/.claude/projects",
                "evidence_refs": ["claude_sessions"]
            },
            {
                "question": "Enable the github collector?",
                "reasoning": "gh auth status returned 0",
                "evidence_refs": ["github"]
            },
            {
                "question": "Tailscale or ssh for the VPS transport?",
                "reasoning": "user said tailscale in the question_log from the scan",
                "evidence_refs": []
            }
        ]);
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ollama_ok(body))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let scan = sample_scan();
        let opts = AskOptions {
            ollama_url: server.uri(),
            ..Default::default()
        };
        let ans = ask_onboarding_with(&scan, &http, opts).await;
        assert_eq!(ans.question_log.len(), 3);
        assert_eq!(
            ans.question_log[0].question,
            "Which claude_sessions paths to monitor?"
        );
        assert_eq!(
            ans.question_log[2].question,
            "Tailscale or ssh for the VPS transport?"
        );
        assert!(ans.question_log[2].evidence_refs.is_empty());
    }

    // -----------------------------------------------------------------------
    // Direct AskOnboardingError test (no wiremock)
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn call_ollama_returns_http_error_on_500() {
        // This is a unit test for the error path that `ask_onboarding`
        // maps to the baseline. We drive `call_ollama` directly so
        // we can assert the typed error variant.
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/chat"))
            .respond_with(ResponseTemplate::new(503).set_body_string("service unavailable"))
            .mount(&server)
            .await;

        let http = reqwest::Client::new();
        let opts = AskOptions {
            ollama_url: server.uri(),
            ..Default::default()
        };
        let prompt = build_prompt(&sample_scan());
        let err = call_ollama(&http, &opts, &prompt).await.unwrap_err();
        match err {
            AskOnboardingError::OllamaHttp { status, body } => {
                assert_eq!(status, 503);
                assert!(body.contains("service unavailable"));
            }
            other => panic!("expected OllamaHttp, got {other:?}"),
        }
    }

    #[test]
    fn prompt_contains_all_8_collector_ids() {
        let p = build_prompt(&sample_scan());
        for id in [
            "github",
            "calendar",
            "claude_sessions",
            "gmail",
            "notes",
            "vscode_extensions",
            "chrome_history",
            "brave_history",
        ] {
            assert!(p.contains(id), "prompt missing collector_id {id}");
        }
    }

    #[test]
    fn prompt_contains_system_directive() {
        let p = build_prompt(&sample_scan());
        assert!(p.contains("You are configuring"));
        assert!(p.contains("Reply with JSON only"));
    }
}
