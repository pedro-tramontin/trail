/**
 * TypeScript mirrors of the Rust serde shapes used by the
 * Phase 6 onboarding IPC commands. Field ordering matches
 * the Rust source (src-tauri/src/onboarding/{scan,answers}.rs)
 * for visual diff-ability. Optional fields use `?` to mirror
 * `#[serde(default, skip_serializing_if = "Option::is_none")]`.
 *
 * These types are consumed by the wizard steps + the
 * `Onboarding.svelte` parent, which chains:
 *   scan_laptop_cmd   -> ScanReport
 *   ask_onboarding_cmd(scan) -> OnboardingAnswers
 *   write_onboarding_config(answers, ssh_key_generated) -> String (path)
 */

// ---------------------------------------------------------------------------
// Phase A — laptop scan (item 6-1, src-tauri/src/onboarding/scan.rs)
// ---------------------------------------------------------------------------

/** Coarse tri-state per-collector. */
export type CollectorStatus =
  | "available"
  | "unavailable"
  | "already_configured";

/** Tagged enum mirroring `EvidenceKind` (tag = "kind", snake_case variants). */
export type EvidenceKind =
  | { kind: "file_exists"; path: string }
  | { kind: "env_var"; name: string; value: string | null }
  | { kind: "dir_exists"; path: string }
  | { kind: "command_exists"; binary: string; path: string }
  | { kind: "macos_app_bundle"; path: string; bundle_id: string };

/** Externally tagged enum mirroring `Platform`
 * (the Rust type in src-tauri/src/onboarding/scan.rs).
 *
 * Note: we use the externally tagged representation (the JSON is
 * `{ "macos": null }` etc., not `{ "os": "macos" }`) because the
 * Rust side has a `Platform::Other(String)` newtype variant that
 * serde's internally-tagged representation (`#[serde(tag = "...")]`)
 * cannot serialize — internally tagged enums forbid newtype variants.
 * See the comment on the Rust `Platform` enum for the long version.
 *
 * The TS shape mirrors the Rust default serde behavior: the variant
 * name is the key, and the variant's payload is the value (null for
 * unit variants, the inner string for `Other`).
 */
export type Platform =
  | { macos: null }
  | { linux: null }
  | { other: string };

/** One candidate collector. `confidence` is `f32` in Rust; JS number is fine. */
export interface CollectorCandidate {
  collector_id: string;
  display_name: string;
  status: CollectorStatus;
  evidence: EvidenceKind;
  confidence: number;
  notes: string | null;
}

export interface ScanReport {
  /** ISO 8601 UTC timestamp from `chrono::DateTime<Utc>`. */
  generated_at: string;
  platform: Platform;
  candidates: CollectorCandidate[];
}

// ---------------------------------------------------------------------------
// Phase B — LLM-driven Q&A (item 6-2, src-tauri/src/onboarding/answers.rs)
// ---------------------------------------------------------------------------

export interface GitHubConfig {
  enabled: boolean;
  repos: string[];
  include_private: boolean;
}

export interface CalendarConfig {
  enabled: boolean;
  ics_paths: string[];
  calendar_app_id?: string | null;
}

export interface VoiceConfig {
  enabled: boolean;
  /** `"tiny" | "base" | "small"` */
  model: string;
  /** `"en" | "pt"` */
  language: string;
}

export interface ReviewTimeConfig {
  /** `"morning" | "evening" | "weekly"` */
  cadence: string;
  /** 0-23 UTC */
  hour_utc: number;
}

export interface SummarizerConfig {
  /** `"ollama" | "stub"` */
  backend: string;
  model: string;
}

export interface TransportConfig {
  /** `"tailscale" | "ssh"` */
  method: string;
  ssh_key_path?: string | null;
}

export interface QuestionLogEntry {
  question: string;
  reasoning: string;
  evidence_refs: string[];
}

/** The downstream-typed answers struct (the Phase C input). */
export interface OnboardingAnswers {
  claude_sessions_paths: string[];
  github: GitHubConfig | null;
  calendar_ics: CalendarConfig | null;
  /** 2026-08-11 — list of browser IDs the user picked on the
   *  Ask step (`chrome`, `brave`, `firefox`, `opera`,
   *  `safari`). Empty list ⇒ no browser history captured.
   *  Mirrors `calendar_ics`'s nullable shape (LLM may
   *  pre-fill, Edit-mode may add/remove). The collector that
   *  reads these files is built in a follow-up PR; for now
   *  this is captured but not consumed. */
  browser_history: string[] | null;
  /** ECD-5 (Layer 1 webcal/ICS URL subscription) — list of
   *  remote `.ics` URLs the user pasted on the new
   *  "Calendar URL" row. `null` means the LLM didn't
   *  pre-fill it (the common case — the LLM is told to
   *  leave it null because the URL list is a per-user-paste
   *  input, not derivable from the scan). Empty list is the
   *  post-edit "no URLs configured" state (the user
   *  explicitly cleared the field). Non-empty lists must
   *  contain only `https://` or `webcal://` URLs — the
   *  wizard's `validate_remote_calendar_url` helper rejects
   *  `http://`, `file://`, and `mailto:` schemes before they
   *  reach this field. Phase C flushes the post-edit list to
   *  `Config.remote_calendar_urls` (see
   *  `src-tauri/src/onboarding/config_writer.rs`). */
  remote_calendar_urls: string[] | null;
  voice: VoiceConfig | null;
  review_time: ReviewTimeConfig;
  summarizer: SummarizerConfig;
  transport: TransportConfig;
  question_log: QuestionLogEntry[];
}

/** Raw ollama chat response. */
export interface OllamaChatResponse {
  model: string;
  message: OllamaChatMessage;
  done?: boolean;
  done_reason?: string | null;
}

export interface OllamaChatMessage {
  role: string;
  content: string;
}

// ---------------------------------------------------------------------------
// LLM-envelope surface — what ollama actually returns, before flattening
// into the downstream `OnboardingAnswers`. Mirrors the schema in
// schemas/onboarding-answer.schema.json.
// ---------------------------------------------------------------------------

export interface AnswerFieldBool {
  selected: boolean;
  notes?: string | null;
  evidence_refs?: string[];
}

export interface AnswerFieldVoice {
  selected: boolean;
  model: string;
  language: string;
  notes?: string | null;
  evidence_refs?: string[];
}

export interface AnswerFieldReviewTime {
  selected: boolean;
  cadence: string;
  hour_utc: number;
  notes?: string | null;
  evidence_refs?: string[];
}

export interface AnswerFieldSummarizer {
  selected: boolean;
  backend: string;
  model: string;
  notes?: string | null;
  evidence_refs?: string[];
}

export interface AnswerFieldTransport {
  selected: boolean;
  method: string;
  notes?: string | null;
  evidence_refs?: string[];
}

export interface OnboardingEnvelope {
  claude_sessions_paths: AnswerFieldBool;
  github: AnswerFieldBool;
  calendar_ics: AnswerFieldBool;
  /** 2026-08-11 — browser-history pick list. The LLM may
   *  pre-fill it (e.g. "you have Chrome installed, enable
   *  chrome_history"); the user can edit it on the Ask
   *  step. Same `AnswerFieldBool` shape as the other
   *  data-source rows so the wizard's per-row tooltip
   *  reasoning keeps working. */
  browser_history: AnswerFieldBool;
  voice: AnswerFieldVoice;
  review_time: AnswerFieldReviewTime;
  summarizer: AnswerFieldSummarizer;
  transport: AnswerFieldTransport;
  question_log: QuestionLogEntry[];
}

// ---------------------------------------------------------------------------
// Phase D — install wizard (item 6-6, will ship later; the wizard step
// only needs the discriminated string the user picks).
// ---------------------------------------------------------------------------

export type InstallOption = "auto" | "show_script" | "skip";

// ---------------------------------------------------------------------------
// Hoisted per-step state (PR #193)
//
// The wizard root (`Onboarding.svelte`) keeps these in `$state` and
// passes them down to the step components. Each step mutates the
// object directly; the parent's reactive graph stays live. This
// is what survives a Back navigation — without hoisting, every
// `$state` declared inside the step component is destroyed when
// Svelte's `{#if}` block unmounts the step.
// ---------------------------------------------------------------------------

/** Step 2 (Ask) state. LLM-fetched `answers` is NOT included —
 *  re-fetching on remount is fast and idempotent. */
export interface StepAskState {
  /** Whether the user is in "Edit" mode (textareas visible). */
  editing: boolean;
  /** Local edit buffer for the claude_sessions paths row. */
  edit_claude_paths: string;
  /** Local edit buffer for the github repos row. */
  edit_github_repos: string;
  /** Local review time, "HH:MM" 24h. Translated to UTC hour on
   *  Next (the Rust scheduler parses UTC). */
  review_hhmm_local: string;
  /** Local edit state for the Voice capture checkbox (mirrors the
   *  github/claude row pattern).
   *
   *  2026-08-11 — defaults flipped to `true` per user feedback:
   *  "it would be nice to have it enabled by default with the
   *  best settings for it." The model picker pre-selects
   *  `base.en` (the "best" v1 default — `tiny.en` is too lossy,
   *  `small.en` is too slow for an always-on capture loop). The
   *  "Save & continue" path now reflects the post-edit state in
   *  the answer row (the previous bug was: edit + flip on + Save
   *  → row still showed "disabled"). */
  edit_voice_enabled: boolean;
  /** Model the user picks on the Voice capture row when in Edit
   *  mode. Defaults match `config_writer.rs` so the on-disk
   *  fallback chain produces the same result whether the LLM
   *  set it or the wizard inferred it. */
  edit_voice_model: string;
  /** 2026-08-11 — Calendar source radio. `"event_kit"` for the
   *  macOS Calendar.app path (new in this PR), `"ics"` for the
   *  legacy `.ics` file path (Linux-only or macOS fallback).
   *  Default depends on the host platform: `event_kit` on
   *  macOS, `ics` on Linux. The Onboarding parent component
   *  picks the default at mount time; StepAsk.svelte's edit
   *  template binds a radio here. The post-edit state is
   *  committed to the `answers.calendar_ics` field via
   *  `build_edited_answers`. */
  edit_calendar_source: "event_kit" | "ics";
  /** Local edit buffer for the Calendar .ics file paths. Only
   *  shown when `edit_calendar_source === "ics"`. Mirrors the
   *  `edit_claude_paths` / `edit_github_repos` pattern: a
   *  newline-separated list rendered as a `<textarea>`. The
   *  `build_edited_answers` commit reads this into
   *  `answers.calendar_ics.ics_paths` when the user picks the
   *  "Custom .ics file" radio. Pre-populated from
   *  `answers.calendar_ics.ics_paths` (or empty when
   *  `calendar_ics` is null) so Edit-mode preserves prior edits.
   *
   *  2026-08-11 — added to fix the missing-file-picker bug:
   *  the user picks "Custom .ics file" but there's no input
   *  to enter the path. The collector previously got an empty
   *  `ics_paths` list and emitted an empty calendar.json. */
  edit_ics_paths: string;
  /** Local edit buffer for the Browser-history pick list
   *  (mirrors the github row pattern). The user picks one or
   *  more browser IDs (`chrome`, `brave`, `firefox`,
   *  `opera`, `safari`) via checkboxes; the rendered list is
   *  newline-separated. Default: empty (none selected — the
   *  user must opt in).
   *
   *  2026-08-11 — added so the Ask step mirrors the scanner's
   *  browser-history probes. The scanner now reports
   *  `chrome_history` / `brave_history` / `firefox_history` /
   *  `opera_history` / `safari_history` candidates; the Ask
   *  step lets the user pick which ones to enable. The actual
   *  data collector is built in a follow-up PR — for now,
   *  the picker is captured in `answers.browser_history` (a
   *  `string[]` of browser IDs) and the config writer
   *  no-ops on it. The PR that builds the collector will
   *  add the corresponding `BrowserSource` enum on
   *  `CollectorLaptopConfig`. */
  edit_browser_history: string;
  /** ECD-5 (Layer 1 webcal/ICS URL subscription) — local
   *  edit buffer for the new "Calendar URL" row. Mirrors the
   *  `edit_claude_paths` / `edit_github_repos` pattern: a
   *  newline-separated list rendered as a `<textarea>`. The
   *  user pastes one `.ics` URL per line. `build_edited_answers`
   *  splits + trims + drops empties, runs each remaining line
   *  through `validate_remote_calendar_url` (rejects
   *  `http://`, `file://`, `mailto:`), and writes the
   *  validated list to `answers.remote_calendar_urls`. Empty
   *  by default (no URLs configured). The LLM is told to
   *  leave the field unset in its answer — `run_ask` does not
   *  pre-populate this buffer from the LLM's response. */
  edit_remote_calendar_urls: string;
}

/** Step 3 (Transport) state. All fields are user-visible
 *  (typed in the form) or transient (test-connection
 *  in-flight flag). */
export interface StepTransportState {
  host: string;
  user: string;
  port: number;
  /** Public key in OpenSSH single-line form. Populated by
   *  `generate_ssh_key` (which is idempotent — re-running it
   *  returns the existing public key) or by the "Use existing
   *  key" path. `null` until one of those has resolved. */
  ssh_key_path: string | null;
  /** How the key was attached. Drives the right-hand
   *  affordance on remount. */
  ssh_key_source: "generated" | "existing" | null;
  /** True while `generate_ssh_key` is in flight. */
  generating: boolean;
  /** Error from the most recent keygen or "Use existing"
   *  attempt. Cleared on the next attempt. */
  key_error: string | null;
  /** "Test connection" button state. */
  test_state: "idle" | "testing" | "ok" | "error";
  test_error: string | null;
}

// ---------------------------------------------------------------------------
// Shared test fixtures — imported by *.test.ts so the wizard steps and
// the parent Onboarding.svelte exercise the same shapes the Rust
// commands return. Keep these in sync with the test mocks in
// `Onboarding.test.ts` and the per-step `*.test.ts` files.
// ---------------------------------------------------------------------------

export const MOCK_SCAN_REPORT: ScanReport = {
  generated_at: "2026-08-02T12:00:00Z",
  platform: { macos: null },
  candidates: [
    {
      collector_id: "github",
      display_name: "GitHub activity",
      status: "available",
      evidence: {
        kind: "command_exists",
        binary: "gh",
        path: "/opt/homebrew/bin/gh",
      },
      confidence: 0.95,
      notes: "gh CLI authenticated",
    },
    {
      collector_id: "claude_sessions",
      display_name: "Claude sessions",
      status: "available",
      evidence: {
        kind: "dir_exists",
        path: "/Users/test/.claude/projects",
      },
      confidence: 0.9,
      notes: null,
    },
    {
      collector_id: "calendar",
      display_name: "Calendar",
      status: "unavailable",
      evidence: { kind: "file_exists", path: "" },
      confidence: 0,
      notes: "no ICS files found",
    },
    {
      collector_id: "voice",
      display_name: "Voice",
      status: "unavailable",
      evidence: { kind: "file_exists", path: "" },
      confidence: 0,
      notes: "no whisper model installed",
    },
  ],
};

export const MOCK_ANSWERS: OnboardingAnswers = {
  claude_sessions_paths: ["/Users/test/.claude/projects"],
  github: { enabled: true, repos: [], include_private: false },
  calendar_ics: null,
  // 2026-08-11 — browser-history picker. Default null in
  // MOCK so the new "Browser history" row shows the
  // "disabled" state in tests (no LLM pre-fill).
  browser_history: null,
  // ECD-5 — calendar URL list. Default null in MOCK so the
  // new "Calendar URL" row shows the "disabled" state in
  // tests (no LLM pre-fill, matching the schema's
  // `Option<Vec<String>>` shape).
  remote_calendar_urls: null,
  voice: null,
  review_time: { cadence: "evening", hour_utc: 18 },
  summarizer: { backend: "stub", model: "stub" },
  transport: { method: "ssh", ssh_key_path: null },
  question_log: [
    {
      question: "Enable github collector?",
      reasoning: "gh CLI is authenticated; safe to enable by default.",
      evidence_refs: ["github"],
    },
    // The mock has calendar_ics = null and voice = null; entries
    // exist for both so the Svelte tooltip can surface the
    // LLM's reasoning for the disabled fields. The evidence_refs
    // must contain the field_id so `find_reason` can locate them.
    {
      question: "Enable the calendar collector?",
      reasoning: "no .ics files or Calendar.app bundle were found on this machine.",
      evidence_refs: ["calendar"],
    },
    {
      question: "Enable voice capture?",
      reasoning: "voice is GPU-bound; defaulting to disabled until the user opts in via Settings.",
      evidence_refs: ["voice"],
    },
  ],
};

/**
 * ECD-5 (Layer 1 webcal/ICS URL subscription) — validate a
 * remote `.ics` URL the user pasted on the new "Calendar
 * URL" row. Returns `true` if the URL is a syntactically
 * well-formed `https://` or `webcal://` URL with a non-empty
 * host; `false` otherwise. The validation is intentionally
 * permissive on the path/query (we don't want to over-fit on
 * specific calendar servers) and strict on the scheme (we
 * must reject `http://` to avoid cleartext fetches, and
 * `file://` / `mailto:` to avoid local-fs reads or mailto
 * handlers). Exported separately from `StepAsk.svelte` so the
 * vitest suite can exercise the edge cases directly without
 * rendering the wizard component.
 *
 * The on-disk Config field accepts `webcal://` unchanged (the
 * Rust side rewrites it to `https://` at fetch time — see
 * `crates/trail-collector/src/collectors/calendar/remote_calendar.rs`),
 * so we accept both schemes here.
 */
export function validate_remote_calendar_url(raw: string): boolean {
  const s = raw.trim();
  if (s.length === 0) return false;
  // Use the URL constructor as the first gate — it parses
  // the scheme + host + path atomically and rejects
  // malformed input (e.g. "not a url"). The constructor
  // accepts both `https://` and `webcal://` because the
  // `webcal:` scheme is a non-special URL but parses without
  // throwing.
  let parsed: URL;
  try {
    parsed = new URL(s);
  } catch {
    return false;
  }
  if (parsed.protocol !== "https:" && parsed.protocol !== "webcal:") {
    return false;
  }
  // Reject "https://" with no host (the URL constructor
  // accepts "https:/path" with an empty host — that's not a
  // calendar URL).
  if (parsed.hostname.length === 0) return false;
  return true;
}
