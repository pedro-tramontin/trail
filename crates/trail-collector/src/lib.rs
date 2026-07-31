//! trail-collector — generic collector for Trail.
//!
//! Phase 1 surface (VPS side): `--once`, `--validate <file>`, `--health` — all
//! driven by `--config <path>`.
//!
//! Phase 2 surface (laptop side, item 2-1): a fourth subcommand `--collect
//! <source> --laptop-config <path>` dispatches to a per-source collector
//! module under `collectors::*`, validates the produced `RawOutput` against a
//! per-source JSON Schema (Draft 2020-12), and writes the result to
//! `~/.trail/raw/<date>/<source>.json`.
//!
//! The collector stays sync (`gh` < 30s, JSONL parse seconds, .ics parse ms).
//! The Tauri orchestrator (§2.5) wraps the binary in
//! `tokio::process::Command::output().await`.

pub mod collect;
pub mod collectors;
pub mod config;
pub mod health;
pub mod once;
pub mod validate;

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "trail-collector",
    version,
    about = "Generic collector for Trail (VPS + laptop surfaces)."
)]
pub struct Cli {
    /// Path to collector.json (the source of truth for all VPS-side paths).
    /// Required for the three Phase 1 modes; ignored for `--collect` (which
    /// reads its own laptop config from `--laptop-config`).
    #[arg(long, required = true)]
    pub config: PathBuf,

    #[command(subcommand)]
    pub mode: Mode,
}

#[derive(Subcommand, Debug)]
pub enum Mode {
    /// Process all pending inbox files, then exit. (Phase 1 §1.8, VPS cron mode.)
    Once,
    /// Schema-check a single file. (Phase 1 §1.7, VPS.)
    Validate {
        /// The file to validate against the configured schema.
        file: PathBuf,
    },
    /// Verify config + paths + schema are sane. (Phase 1 §1.6, VPS.)
    Health,
    /// Run one source's collector + validate + write raw JSON. (Phase 2 §2.1.)
    Collect {
        /// The source to run. One of `github | claude-sessions | calendar`.
        #[arg(long, value_enum)]
        source: collectors::Source,
        /// Path to the laptop-side collector config (the `~/.trail/config.json`
        /// extended slice — `source`, `github`, `claude_sessions_paths`,
        /// `calendar_ics`, `raw_root`, `schema_path`).
        #[arg(long)]
        laptop_config: PathBuf,
    },
}
