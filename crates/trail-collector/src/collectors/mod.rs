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
}

impl Source {
    /// On-disk / raw-JSON naming. Underscores everywhere — see the master
    /// plan's frozen `~/.trail/raw/<date>/<source>.json` contract.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::ClaudeSessions => "claude_sessions",
            Self::Calendar => "calendar",
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
            _ => None,
        }
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
    pub calendar_ics: std::path::PathBuf,
    pub raw_root: std::path::PathBuf,
    pub schema_path: std::path::PathBuf,
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
    }
}

pub mod calendar;
pub mod claude_sessions;
pub mod github;
