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

/** Tagged enum mirroring `Platform` (tag = "os", snake_case variants). */
export type Platform =
  | { os: "macos" }
  | { os: "linux" }
  | { os: "other"; os_name: string };

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
// Shared test fixtures — imported by *.test.ts so the wizard steps and
// the parent Onboarding.svelte exercise the same shapes the Rust
// commands return. Keep these in sync with the test mocks in
// `Onboarding.test.ts` and the per-step `*.test.ts` files.
// ---------------------------------------------------------------------------

export const MOCK_SCAN_REPORT: ScanReport = {
  generated_at: "2026-08-02T12:00:00Z",
  platform: { os: "macos" },
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
  voice: null,
  review_time: { cadence: "evening", hour_utc: 22 },
  summarizer: { backend: "stub", model: "stub" },
  transport: { method: "ssh", ssh_key_path: null },
  question_log: [
    {
      question: "Enable github collector?",
      reasoning: "gh CLI is authenticated; safe to enable by default.",
      evidence_refs: ["github"],
    },
  ],
};
