//! Core `summarize-day` pipeline.
//!
//! Reads each day's collector JSON from `~/.trail/raw/<date>/*.json`,
//! builds a single user prompt, hands it to the local [`crate::ollama::OllamaClient`]
//! for generation, scrubs the model's response through the (shim)
//! anonymizer, validates the five required `##` sections are present,
//! writes the draft to `~/.trail/drafts/<date>.md`, and returns a
//! [`SummarizeReceipt`].
//!
//! The five required sections — `## Summary`, `## Wins`, `## Blockers`,
//! `## People`, `## Open threads` — are baked into [`REQUIRED_SECTIONS`]
//! and gated by [`crate::prompts::SYSTEM_PROMPT`]. The LLM downstream
//! consumers (the VPS review UI, the daily-check-in notifier) assume
//! exactly those headers, in that order, with nothing before/after.
//! A missing header is a [`SummarizerError::MissingSection`]; the
//! caller is expected to surface that to the user so they can re-run.
//!
//! Anonymization is delegated to [`crate::anonymizer::anonymize`] (a
//! no-op shim today; item 3-3 fills in the regex scrubber).
//!
//! ## Error semantics
//!
//! * [`SummarizerError::NoRawFiles`] — no `raw/<date>/*.json` files were
//!   found. Distinct from "the date dir doesn't exist" / "the dir exists
//!   but has no JSON" — both surface as `NoRawFiles(date)` so the
//!   frontend can show one clear "no captures for that day" message.
//! * [`SummarizerError::MissingSection`] — the model returned text but
//!   at least one required `##` header is missing. The draft is NOT
//!   written; the caller can decide whether to retry with a different
//!   model or a stronger prompt.
//! * [`SummarizerError::Ollama`], [`SummarizerError::Io`],
//!   [`SummarizerError::Json`] — propagated from the underlying call so
//!   the caller can branch on the typed variant.
//!
//! ## Bootstrap
//!
//! `bootstrap_count` on the receipt is always `0` in v1. Item 3-4 will
//! add a learning module that re-injects prior preferences into the
//! user prompt; once that lands the field starts reporting nonzero
//! counts.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::anonymizer::anonymize;
use crate::ollama::{OllamaClient, OllamaError};
use crate::prompts::{SYSTEM_PROMPT, USER_PROMPT_TEMPLATE};

/// Receipt returned by [`run`] once a draft has been written. The
/// frontend serializes this directly via `serde::Serialize` so a
/// success toast can show "drafted `~/.trail/drafts/<date>.md` from
/// `<N>` sources (model `<model>`)".
#[derive(Debug, Clone, serde::Serialize)]
pub struct SummarizeReceipt {
    /// Date the draft summarizes, in `YYYY-MM-DD` form (matches the
    /// `raw/<date>` and `drafts/<date>.md` path conventions).
    pub date: String,
    /// Ollama model name used for this summarization (e.g. `llama3`).
    pub model: String,
    /// Source names derived from `raw/<date>/<file>.json` file stems
    /// (e.g. `["github", "calendar"]`). Sorted alphabetically because
    /// we collect into a [`BTreeMap`] before serializing.
    pub raw_sources: Vec<String>,
    /// Absolute path to the draft file written by this run.
    pub draft_path: PathBuf,
    /// How many bootstrap rules were in scope when the run started.
    /// Always `0` in v1; item 3-4 (learner) raises this when prior
    /// preferences are re-injected.
    pub bootstrap_count: usize,
    /// The five `##` headers that parsed (in [`REQUIRED_SECTIONS`]
    /// order). Surfaced to the UI so a future "preview sections" view
    /// can light up green/red per header without re-parsing the draft.
    pub sections_found: Vec<String>,
}

/// Typed errors from the summarizer. All variants implement
/// [`std::error::Error`] via `thiserror`'s derive; the string error
/// trait on the Tauri command side (`String`) is the only consumer
/// flattening needed.
#[derive(Debug, thiserror::Error)]
pub enum SummarizerError {
    #[error("ollama error: {0}")]
    Ollama(#[from] OllamaError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json parse error: {0}")]
    Json(#[from] serde_json::Error),
    /// A required `## ` section header was missing from the LLM's
    /// response, or appeared in the wrong order. The wrapped
    /// string is the expected next header (e.g. `"## Blockers"`).
    #[error("missing required section in LLM response: {0}")]
    MissingSection(String),
    /// An unexpected `## ` section header appeared AFTER all five
    /// required ones. The wrapped string is the offending header
    /// (e.g. `"## Notes"`). Distinct from `MissingSection` so the
    /// caller can surface a clearer error ("got an extra section
    /// at the end" instead of "missing a section").
    #[error("unexpected extra section in LLM response after all required: {0}")]
    UnexpectedSection(String),
    #[error("no raw collector files found for date {0}")]
    NoRawFiles(String),
}

/// The five `##` headers the LLM is required to emit, in canonical
/// order. Compared as plain substrings against the scrubbed response —
/// enough for v1 because the system prompt forbids any other `##`
/// header outside the five. (A future "loose parser" could split on
/// `^## ` line boundaries; not needed yet.)
pub const REQUIRED_SECTIONS: &[&str] = &[
    "## Summary",
    "## Wins",
    "## Blockers",
    "## People",
    "## Open threads",
];

/// Read `~/.trail/raw/<date>/*.json`, call ollama, scrub, validate
/// the five headers, write `~/.trail/drafts/<date>.md`, return the
/// receipt. See module docs for error semantics.
///
/// The `date` argument is the date folder under `raw_root` (e.g.
/// `"2026-07-29"`) and the stem of the draft filename (e.g.
/// `2026-07-29.md`). No date validation here — the caller is
/// responsible for any `YYYY-MM-DD` format check.
///
/// `strictness` is the anonymization level (`"off" | "moderate" |
/// "aggressive"`), passed through to [`anonymize`]. Currently a
/// no-op in v1; the value is preserved in the call so item 3-3
/// (anonymizer real impl) doesn't need a signature change.
///
/// `bootstrap_path` is the absolute path to the `summary_bootstrap.json`
/// file the learner maintains (see [`crate::learner`]). When the file
/// exists, its rules are rendered as a Markdown block and injected into
/// the user prompt just below the "Context for the day's schedule:"
/// header — so the model sees the user's prior preferences as few-shot
/// context. When the file is missing or empty, the `{bootstrap}`
/// placeholder is replaced with the empty string (no behavioural
/// change vs. v1).
///
/// `client` is a pre-built [`OllamaClient`]; tests pass a `wiremock`
/// server's URI while production code constructs one with
/// [`OllamaClient::new`] against the default endpoint.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    raw_root: &Path,
    drafts_dir: &Path,
    bootstrap_path: &Path,
    date: &str,
    model: &str,
    strictness: &str,
    rules: &[crate::anonymizer::AnonymizationRule],
    client: &OllamaClient,
) -> Result<SummarizeReceipt, SummarizerError> {
    // 1. Locate the day's folder. We treat "directory missing" and
    //    "directory exists but empty" identically — both surface as
    //    NoRawFiles(date) so the frontend shows one message instead
    //    of two slightly-different ones.
    let day_dir = raw_root.join(date);
    let mut raw_payloads: Vec<(String, serde_json::Value)> = Vec::new();
    if day_dir.exists() {
        for entry in std::fs::read_dir(&day_dir)? {
            let entry = entry?;
            let path = entry.path();
            // Skip non-json entries (`.DS_Store`, swapfiles, etc.) —
            // Phase 2 collectors all emit `.json`.
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let source = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let bytes = std::fs::read(&path)?;
            let value: serde_json::Value = serde_json::from_slice(&bytes)?;
            raw_payloads.push((source, value));
        }
    }
    if raw_payloads.is_empty() {
        return Err(SummarizerError::NoRawFiles(date.to_string()));
    }

    // 2. Build the user prompt. We sort sources alphabetically (via
    //    BTreeMap) so the JSON the LLM sees is deterministic across
    //    runs of the same day — makes diffing the prompt during
    //    prompt-iteration debugging tractable.
    let by_source: BTreeMap<String, serde_json::Value> = raw_payloads.into_iter().collect();
    let raw_data_json = serde_json::to_string_pretty(&by_source)?;
    // 2026-08-11 — DO NOT anonymize the *input* payload.
    // Privacy is enforced by:
    //   1. The collector-side allowlist (only the schema's
    //      declared fields are captured; `DESCRIPTION`/`COMMENT`
    //      /`X-ALT-DESC` and `EKEvent.notes`-too-sensitive are
    //      never read or are routed through the LLM's own
    //      judgment).
    //   2. The *local* LLM (Ollama) is the trusted anonymizer —
    //      it sees the raw payload and is instructed by the
    //      system prompt to redact PII (names, emails, phone
    //      numbers) before emitting the five required sections.
    //      The summarizer's only post-LLM scrub is the output
    //      regex pass on `anonymize(&raw_response, ...)`.
    //   3. The summarizer's output-side `anonymize` catches any
    //      patterns the LLM failed to redact (defense in depth).
    // Adding an input-side scrub here would hide content from
    // the LLM that it might legitimately need (e.g. an
    // event summary that itself names the people involved),
    // reducing summary quality without improving privacy —
    // the LLM already has the raw input.
    // Render the learner bootstrap as a Markdown block. Returns `None`
    // when the file is missing or the rules list is empty; in both
    // cases the placeholder collapses to the empty string, preserving
    // the v1 prompt shape. I/O or JSON errors are logged but
    // downgraded to "no bootstrap" so a corrupt/missing bootstrap
    // file doesn't take down the whole summarizer run — the user
    // just loses the few-shot context, not the day's summary.
    let bootstrap_block_text = match crate::learner::bootstrap_block(bootstrap_path) {
        Ok(Some(block)) => block,
        Ok(None) => String::new(),
        Err(e) => {
            eprintln!(
                "summarizer: bootstrap_block failed at {}: {} (continuing without bootstrap)",
                bootstrap_path.display(),
                e
            );
            String::new()
        }
    };
    let user_prompt = USER_PROMPT_TEMPLATE
        .replace("{date}", date)
        .replace("{bootstrap}", &bootstrap_block_text)
        .replace("{raw_data_json}", &raw_data_json);

    // 3. Call ollama. Any ollama-layer failure (network down, model
    //    404, empty model response) bubbles up as Ollama(_) via the
    //    `#[from] OllamaError` conversion on `SummarizerError`.
    let raw_response = client.generate(SYSTEM_PROMPT, &user_prompt, model).await?;

    // 4. Anonymize. The shim is identity today; once item 3-3 lands
    //    this becomes the regex scrubber gated on `strictness`.
    let scrubbed = anonymize(&raw_response, strictness, rules);

    // 5. Validate all five required sections are present, in order,
    //    with no extras. Pre-fix the check was `scrubbed.contains(header)`
    //    for each required header, which accepted:
    //      * out-of-order sections (e.g. Blockers before Wins)
    //      * duplicate headers (e.g. two `## Summary` blocks)
    //      * extra `## ...` headers (e.g. an unwanted `## Notes`)
    //    The fix walks through `scrubbed` once and asserts each
    //    required header appears in `REQUIRED_SECTIONS` order, with
    //    no other `## ` headers between any two required ones.
    //
    // `REQUIRED_SECTIONS` entries include the `## ` prefix (they
    // are also matched against the raw line in the legacy `contains`
    // form). For line-by-line matching we derive the stripped
    // header text on the fly instead of a parallel hardcoded list,
    // so the validator cannot drift from the canonical list if
    // someone edits `REQUIRED_SECTIONS` later. This is computed
    // once via `OnceLock` (cheap; the body runs once per call).
    //
    // Three failure modes map to two errors:
    //   * header appears when the next required one was missing /
    //     appeared out of order → `MissingSection(<expected>)`.
    //   * header appears after all 5 required have been seen →
    //     `UnexpectedSection(<actual>)`.
    //   * loop ended before all 5 required were seen →
    //     `MissingSection(<expected>)`.
    fn required_header_text(idx: usize) -> String {
        REQUIRED_SECTIONS[idx]
            .strip_prefix("## ")
            .unwrap_or(REQUIRED_SECTIONS[idx])
            .to_string()
    }
    let mut idx = 0usize;
    for line in scrubbed.lines() {
        if let Some(header) = line.strip_prefix("## ") {
            // Trim trailing whitespace and any trailing `#` chars
            // (some LLMs add a trailing `## Summary ##`).
            let header = header.trim().trim_end_matches('#').trim();
            if idx < REQUIRED_SECTIONS.len() && header == required_header_text(idx) {
                idx += 1;
            } else if idx >= REQUIRED_SECTIONS.len() {
                // We already saw all 5 required headers; this
                // extra `## ...` line is the unexpected one.
                return Err(SummarizerError::UnexpectedSection(format!("## {header}")));
            } else {
                // The section we expected is missing (or appeared
                // out of order) — surface the EXPECTED next header.
                return Err(SummarizerError::MissingSection(format!(
                    "## {}",
                    required_header_text(idx)
                )));
            }
        }
    }
    if idx < REQUIRED_SECTIONS.len() {
        return Err(SummarizerError::MissingSection(format!(
            "## {}",
            required_header_text(idx)
        )));
    }
    let sections_found: Vec<String> = REQUIRED_SECTIONS.iter().map(|s| (*s).to_string()).collect();

    // 6. Write the draft. `create_dir_all` is idempotent, so the
    //    happy-path launch doesn't need a separate bootstrap step.
    std::fs::create_dir_all(drafts_dir)?;
    let draft_path = drafts_dir.join(format!("{date}.md"));
    std::fs::write(&draft_path, &scrubbed)?;

    // Sources come from the BTreeMap order, so already alphabetical.
    let raw_sources = by_source.into_keys().collect();
    Ok(SummarizeReceipt {
        date: date.to_string(),
        model: model.to_string(),
        raw_sources,
        draft_path,
        bootstrap_count: 0,
        sections_found,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A canned 5-section Markdown response. Every required header is
    /// present so the happy-path tests pass validation.
    const CANNED_FIVE_SECTIONS: &str = "## Summary\n\nTested summarizer end-to-end with two collectors.\n\n## Wins\n\n- Wrote fixtures\n- Wired ollama mock\n\n## Blockers\n\nNone\n\n## People\n\n- [PM] for review feedback\n\n## Open threads\n\n- Land item 3-3 (anonymizer) next.\n";

    /// Bootstrap path used by tests — a path under `/tmp` that is
    /// guaranteed not to exist. `learner::bootstrap_block` returns
    /// `None` for missing files, so the `{bootstrap}` placeholder
    /// collapses to `""` exactly like the v1 behaviour.
    fn test_bootstrap_path(tmp: &TempDir) -> std::path::PathBuf {
        tmp.path().join("nonexistent-summary-bootstrap.json")
    }

    /// Same shape as CANNED_FIVE_SECTIONS but with `## People` removed,
    /// for the missing-section test.
    const CANNED_MISSING_PEOPLE: &str = "## Summary\n\nA day.\n\n## Wins\n\n- One thing.\n\n## Blockers\n\nNone\n\n## Open threads\n\n- Revisit.\n";

    /// Sections in the wrong order (Blockers before Wins). Pre-fix
    /// `contains` would have accepted this; the new strict validator
    /// must reject it.
    const CANNED_OUT_OF_ORDER: &str = "## Summary\n\nA day.\n\n## Blockers\n\nNone\n\n## Wins\n\n- One thing.\n\n## People\n\n- [PM]\n\n## Open threads\n\n- Revisit.\n";

    /// Duplicate `## Summary` header. Pre-fix `contains` accepted
    /// this; the new strict validator must reject it.
    const CANNED_DUPLICATE_HEADER: &str = "## Summary\n\nA day.\n\n## Summary\n\nAgain.\n\n## Wins\n\n- One thing.\n\n## Blockers\n\nNone\n\n## People\n\n- [PM]\n\n## Open threads\n\n- Revisit.\n";

    /// Extra `## Notes` section between required ones. Pre-fix
    /// `contains` accepted this; the new strict validator must
    /// reject it.
    const CANNED_EXTRA_SECTION: &str = "## Summary\n\nA day.\n\n## Notes\n\nExtra section that shouldn't be here.\n\n## Wins\n\n- One thing.\n\n## Blockers\n\nNone\n\n## People\n\n- [PM]\n\n## Open threads\n\n- Revisit.\n";

    /// Extra `## Notes` section AFTER all five required (i.e.
    /// appended at the bottom). Pre-fix this returned
    /// `MissingSection("## <end of required sections>")`, which
    /// was opaque and hid the actual offending header. The fix
    /// surfaces the header via a dedicated `UnexpectedSection`
    /// error so the operator can see what the LLM added.
    const CANNED_EXTRA_SECTION_AFTER_ALL: &str = "## Summary\n\nA day.\n\n## Wins\n\n- One thing.\n\n## Blockers\n\nNone\n\n## People\n\n- [PM]\n\n## Open threads\n\n- Revisit.\n\n## Notes\n\nExtra section that shouldn't be here.\n";

    /// Write a single `raw/<date>/<name>.json` fixture file into the
    /// given tempdir. The contents are minimal Phase-2-shape JSON; the
    /// LLM never actually parses them in tests because the wiremock
    /// response is canned.
    fn write_raw_fixture(dir: &Path, date: &str, name: &str) {
        let day_dir = dir.join(date);
        std::fs::create_dir_all(&day_dir).expect("create day dir");
        let path = day_dir.join(format!("{name}.json"));
        let mut f = std::fs::File::create(&path).expect("create fixture file");
        let body = format!(
            r#"{{"source":"{name}","captured_at":"{date}T18:00:00Z","date":"{date}","payload":{{"placeholder":true}}}}"#
        );
        f.write_all(body.as_bytes()).expect("write fixture");
    }

    /// Set up wiremock to answer `/api/generate` with `body`. Returns
    /// the running mock server so the test builds an `OllamaClient`
    /// pointed at it.
    async fn ollama_mock_returning(body: &'static str) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": body,
                "done": true,
            })))
            .mount(&server)
            .await;
        server
    }

    /// Test 1 — happy path with 2 fixture files. Verifies the draft is
    /// written, all 5 section headers survive the round-trip, the
    /// receipt's `raw_sources` lists both source names (alphabetical),
    /// and the receipt's `bootstrap_count` is 0 (v1 invariant).
    #[tokio::test]
    async fn summarizer_writes_draft_with_five_required_sections() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");
        write_raw_fixture(&raw_root, "2026-07-29", "calendar");

        let server = ollama_mock_returning(CANNED_FIVE_SECTIONS).await;
        let client = OllamaClient::new(server.uri());

        let receipt = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect("happy path");

        // Receipt sanity.
        assert_eq!(receipt.date, "2026-07-29");
        assert_eq!(receipt.model, "llama3");
        assert_eq!(receipt.bootstrap_count, 0);
        assert_eq!(receipt.raw_sources, vec!["calendar", "github"]);
        assert_eq!(receipt.sections_found.len(), 5);

        // Draft file landed where expected and contains all 5 headers.
        let draft = std::fs::read_to_string(&receipt.draft_path).expect("draft file written");
        for header in REQUIRED_SECTIONS {
            assert!(
                draft.contains(header),
                "draft missing header {header}; full draft:\n{draft}"
            );
        }
    }

    /// Test 2 — non-existent date folder surfaces as `NoRawFiles`.
    /// We point `raw_root` at an empty tempdir so `raw/2099-01-01`
    /// doesn't exist.
    #[tokio::test]
    async fn summarizer_returns_no_raw_files_when_date_dir_missing() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        // Intentionally do NOT create raw_root/2099-01-01.
        let server = ollama_mock_returning(CANNED_FIVE_SECTIONS).await;
        let client = OllamaClient::new(server.uri());

        let err = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2099-01-01",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect_err("missing date dir should fail");
        match err {
            SummarizerError::NoRawFiles(d) => assert_eq!(d, "2099-01-01"),
            other => panic!("expected NoRawFiles, got {other:?}"),
        }
    }

    /// Test 3 — model omits `## People` → we surface MissingSection and
    /// DO NOT write a draft (so a retry doesn't accidentally
    /// double-write on the next attempt).
    #[tokio::test]
    async fn summarizer_returns_missing_section_when_llm_omits_header() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");

        let server = ollama_mock_returning(CANNED_MISSING_PEOPLE).await;
        let client = OllamaClient::new(server.uri());

        let err = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect_err("missing section should fail");
        match err {
            SummarizerError::MissingSection(h) => assert_eq!(h, "## People"),
            other => panic!("expected MissingSection(\"## People\"), got {other:?}"),
        }

        // Drafts dir must not contain the day file (no partial writes).
        let draft_path = drafts_dir.join("2026-07-29.md");
        assert!(
            !draft_path.exists(),
            "draft should NOT exist when validation fails; found: {}",
            draft_path.display()
        );
    }

    /// Test 4 — receipt's `raw_sources` exactly matches the fixture
    /// file stems (alphabetical), and `bootstrap_count` is 0.
    #[tokio::test]
    async fn summarizer_receipt_contains_raw_sources_and_zero_bootstrap_count() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");
        write_raw_fixture(&raw_root, "2026-07-29", "calendar");
        write_raw_fixture(&raw_root, "2026-07-29", "voice");

        let server = ollama_mock_returning(CANNED_FIVE_SECTIONS).await;
        let client = OllamaClient::new(server.uri());

        let receipt = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect("happy path");

        assert_eq!(receipt.raw_sources, vec!["calendar", "github", "voice"]);
        assert_eq!(receipt.bootstrap_count, 0);
    }

    /// Test 5 — ollama returns 500 → the error propagates through
    /// `#[from] OllamaError` and surfaces as `SummarizerError::Ollama`.
    /// No draft is written.
    #[tokio::test]
    async fn summarizer_propagates_ollama_error() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .respond_with(ResponseTemplate::new(500).set_body_string("model exploded"))
            .mount(&server)
            .await;
        let client = OllamaClient::new(server.uri());

        let err = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect_err("500 should fail");
        match err {
            SummarizerError::Ollama(OllamaError::Http(msg)) => {
                assert!(
                    msg.contains("500"),
                    "expected '500' in ollama HTTP error, got: {msg}"
                );
            }
            other => panic!("expected Ollama(Http(_)), got {other:?}"),
        }

        let draft_path = drafts_dir.join("2026-07-29.md");
        assert!(
            !draft_path.exists(),
            "draft must not be written when ollama fails; found: {}",
            draft_path.display()
        );
    }

    /// Sections in the wrong order (Blockers before Wins) must
    /// reject. Pre-fix `contains` would have accepted.
    #[tokio::test]
    async fn summarizer_rejects_out_of_order_sections() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");

        let server = ollama_mock_returning(CANNED_OUT_OF_ORDER).await;
        let client = OllamaClient::new(server.uri());

        let err = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect_err("out-of-order sections must fail validation");
        assert!(
            matches!(err, SummarizerError::MissingSection(_)),
            "expected MissingSection, got {err:?}"
        );
        // The draft must not be written.
        assert!(!drafts_dir.join("2026-07-29.md").exists());
    }

    /// Duplicate `## Summary` header must reject.
    #[tokio::test]
    async fn summarizer_rejects_duplicate_header() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");

        let server = ollama_mock_returning(CANNED_DUPLICATE_HEADER).await;
        let client = OllamaClient::new(server.uri());

        let err = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect_err("duplicate header must fail validation");
        assert!(matches!(err, SummarizerError::MissingSection(_)));
    }

    /// Extra `## Notes` section between required ones. Pre-fix
    /// `contains` accepted this; the new strict validator must
    /// reject it as a missing-in-order section (the next
    /// expected header after `## Summary` is `## Wins`, but the
    /// LLM jumped to `## Notes`, so the error is `MissingSection`
    /// not `UnexpectedSection`).
    #[tokio::test]
    async fn summarizer_rejects_extra_section() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");

        let server = ollama_mock_returning(CANNED_EXTRA_SECTION).await;
        let client = OllamaClient::new(server.uri());

        let err = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect_err("extra (between) section must fail validation");
        match err {
            SummarizerError::MissingSection(h) => {
                assert_eq!(h, "## Wins", "expected next required header");
            }
            other => panic!("expected MissingSection, got {other:?}"),
        }
    }

    /// Extra `## Notes` section appended AFTER all five required
    /// ones. Pre-fix this returned `MissingSection("## <end of
    /// required sections>")`, which hid the actual offending
    /// header. The new validator must surface the header via the
    /// `UnexpectedSection` variant.
    #[tokio::test]
    async fn summarizer_rejects_extra_section_after_all_required() {
        let tmp = TempDir::new().expect("tempdir");
        let raw_root = tmp.path().join("raw");
        let drafts_dir = tmp.path().join("drafts");
        write_raw_fixture(&raw_root, "2026-07-29", "github");

        let server = ollama_mock_returning(CANNED_EXTRA_SECTION_AFTER_ALL).await;
        let client = OllamaClient::new(server.uri());

        let err = run(
            &raw_root,
            &drafts_dir,
            &test_bootstrap_path(&tmp),
            "2026-07-29",
            "llama3",
            "moderate",
            &[],
            &client,
        )
        .await
        .expect_err("extra (after all required) section must fail validation");
        match err {
            SummarizerError::UnexpectedSection(h) => {
                assert_eq!(
                    h, "## Notes",
                    "the unexpected-section variant must surface the offending header"
                );
            }
            other => panic!("expected UnexpectedSection, got {other:?}"),
        }
        // The draft must not be written.
        assert!(!drafts_dir.join("2026-07-29.md").exists());
    }
}
