//! Per-source collector modules. Each exposes `pub fn run(cfg:
//! &CollectorLaptopConfig) -> Result<RawOutput>`. Phase 2 §2.2, §2.3, §2.4 fill
//! in the real `github` / `claude_sessions` / `calendar` implementations — this
//! file ships first so the supervisor in `crate::collect` has a stable dispatch
//! shape to compile against.

use anyhow::Result;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// The single source the supervisor dispatches to per invocation. The CLI
/// `ValueEnum` derive produces clap's `--help`-time subcommand enumeration and
/// the `FromStr` impl for `--collect <source>`.
#[derive(Debug, Clone, Copy, ValueEnum, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Github,
    /// CLI flag is `claude-sessions` (hyphen); serde + the on-disk/raw-output
    /// naming use `claude_sessions` (underscore) — `as_str()` does the
    /// conversion for runtime paths; `#[clap(name = ...)]` overrides the CLI
    /// flag spelling.
    #[clap(name = "claude-sessions")]
    ClaudeSessions,
    Calendar,
    /// Browser history (Chrome, Brave, Opera, Firefox, Safari).
    /// CLI flag is `browser-history`; serde + raw-output use
    /// `browser_history`. See plan
    /// `.hermes/plans/2026-08-11_browser-history-collector.md` for
    /// the architecture. The single collector subprocess reads
    /// every picked browser (configured via
    /// `CollectorLaptopConfig::browser_history`) and emits a
    /// unified `browser_history.json` payload.
    #[clap(name = "browser-history")]
    BrowserHistory,
}

impl Source {
    /// On-disk / raw-JSON naming. Underscores everywhere — see the master
    /// plan's frozen `~/.trail/raw/<date>/<source>.json` contract.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::ClaudeSessions => "claude_sessions",
            Self::Calendar => "calendar",
            Self::BrowserHistory => "browser_history",
        }
    }

    /// Per-source schema filename. Item 2-2+ writes the actual schemas; this
    /// lookup is what the supervisor would use to auto-locate the schema file
    /// if `schema_path` is the parent directory.
    pub fn schema_filename(&self) -> &'static str {
        match self {
            Self::Github => "github.schema.json",
            Self::ClaudeSessions => "claude_sessions.schema.json",
            Self::Calendar => "calendar.schema.json",
            Self::BrowserHistory => "browser_history.schema.json",
        }
    }

    /// Read the `source` field out of a serialised raw output. Used by the
    /// supervisor's validator so the error message names the source even
    /// when the payload shape is wrong.
    pub fn from_payload_value(v: &serde_json::Value) -> Option<Self> {
        let s = v.get("source")?.as_str()?;
        match s {
            "github" => Some(Self::Github),
            "claude_sessions" => Some(Self::ClaudeSessions),
            "calendar" => Some(Self::Calendar),
            "browser_history" => Some(Self::BrowserHistory),
            _ => None,
        }
    }
}

/// The calendar collector's per-invocation choice of data source.
///
/// `Ics` reads a `.ics` file path (today's behaviour, also the
/// macOS fallback when EventKit TCC is denied).
///
/// `EventKit` is macOS-only: the collector initialises an
/// `EKEventStore`, requests full-access to events (Sonoma+), and
/// enumerates today's events via `predicateForEvents`. The tagged
/// enum mirrors the on-disk `Config.calendar.kind` field — the
/// Tauri side serialises one variant, the collector deserialises
/// the same.
///
/// The `EventKit` arm is a no-op on non-macOS targets — the
/// `calendar` module's `run` only matches the arm behind
/// `#[cfg(target_os = "macos")]`. A user who picks EventKit on
/// Linux is rejected at config-validation time before the
/// collector subprocess even spawns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarSourceChoice {
    Ics,
    EventKit,
}

impl Default for CalendarSourceChoice {
    /// Default to `Ics` for both Linux (only choice) and macOS
    /// legacy configs. The wizard flips the macOS default to
    /// `EventKit` after the user explicitly opts in via the
    /// Ask step's "Calendar source" radio.
    fn default() -> Self {
        Self::Ics
    }
}

/// The per-source collector's structured output. Serialized as JSON before
/// schema validation (and so before write). `Deserialize` lets the on-disk
/// round-trip in the supervisor test (and is a no-cost derive — payload is
/// already a `serde_json::Value`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawOutput {
    pub source: String,
    pub captured_at: chrono::DateTime<chrono::Utc>,
    pub date: chrono::NaiveDate,
    pub payload: serde_json::Value,
}

/// The laptop-side per-collector config, distinct from the VPS
/// `CollectorConfig` (which lives in `crate::config`). Source-scoped so each
/// collector gets just the slice it needs; loaded by §2.5's orchestrator or
/// directly via `--laptop-config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorLaptopConfig {
    pub source: Source,
    pub github: GithubLaptopConfig,
    pub claude_sessions_paths: Vec<std::path::PathBuf>,
    /// Calendar data source choice (Ics file path or macOS EventKit).
    /// Defaults to `Ics` so legacy `LaptopCfg` blobs that don't carry
    /// the new field still parse.
    #[serde(default)]
    pub calendar_source: CalendarSourceChoice,
    /// Legacy single-path field, kept for back-compat with
    /// `LaptopCfg` blobs the Tauri orchestrator wrote before
    /// `calendar_source` landed. The dispatch in
    /// `collectors/calendar/mod.rs` always reads `calendar_ics`
    /// when the source is `Ics`, so the legacy blob path still
    /// works.
    pub calendar_ics: std::path::PathBuf,
    /// Optional calendar-name filter for the EventKit source.
    /// `None` ⇒ all calendars the user granted access to. A
    /// `Some(vec)` ⇒ only events whose `EKCalendar.title()`
    /// matches one of the names. Used by `collectors/calendar/eventkit.rs`
    /// on macOS only; the Ics path ignores it (the .ics
    /// already contains the calendar name in its `X-WR-CALNAME`
    /// header).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_names: Option<Vec<String>>,
    pub raw_root: std::path::PathBuf,
    pub schema_path: std::path::PathBuf,
    /// Browser-history collector input — which browsers the user
    /// picked (intersected with the scanner's `Available` set),
    /// plus the resolved DB paths from the scanner. Set to an
    /// empty `BrowserHistoryInput` (default) when the source is
    /// not `BrowserHistory`. See plan
    /// `.hermes/plans/2026-08-11_browser-history-collector.md`.
    #[serde(default)]
    pub browser_history: browser_history::BrowserHistoryInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GithubLaptopConfig {
    pub mode: String,
    pub host: String,
    pub enabled: bool,
}

/// Dispatch to the per-source implementation. Each module's `run` lives next
/// door in this `collectors/` directory; if a v2 adds a fifth source, it's one
/// new mod + one new arm here + one new `ValueEnum` variant.
pub fn dispatch(cfg: &CollectorLaptopConfig) -> Result<RawOutput> {
    match cfg.source {
        Source::Github => github::run(cfg),
        Source::ClaudeSessions => claude_sessions::run(cfg),
        Source::Calendar => calendar::run(cfg),
        Source::BrowserHistory => browser_history::run(&cfg.browser_history),
    }
}

pub mod browser_history;
pub mod calendar;
pub mod claude_sessions;
pub mod github;
// `synth_github` and `synth_claude` are the pure-JSON helpers next door
// to their respective collector modules; not part of the dispatch surface,
// so they're not in the `pub mod X` trio at the bottom of this file by
// convention. `pub(crate)` keeps them visible to the per-source tests
// (`super::super::synth_X::synthesize`) without exposing them on the
// library's public surface.
#[allow(unused_imports, dead_code)]
pub(crate) mod synth_calendar;
#[allow(unused_imports, dead_code)]
pub(crate) mod synth_browser_history;
#[allow(unused_imports, dead_code)]
pub(crate) mod synth_claude;
#[allow(unused_imports, dead_code)]
pub(crate) mod synth_github;
