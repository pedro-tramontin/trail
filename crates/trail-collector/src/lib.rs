//! trail-collector — generic VPS-side collector for Trail.
//!
//! nginx/envoy pattern: the binary is a single, versioned, configuration-driven
//! program. All paths come from `--config <path>`. There is NO env-var fallback
//! inside the binary, NO `~/` expansion, NO hardcoded defaults. The config file
//! is the source of truth.

pub mod config;
pub mod health;
pub mod once;
pub mod validate;

use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "trail-collector",
    version,
    about = "Generic VPS-side collector for Trail."
)]
pub struct Cli {
    /// Path to collector.json (the source of truth for all paths).
    #[arg(long, required = true)]
    pub config: std::path::PathBuf,

    #[command(subcommand)]
    pub mode: Mode,
}

#[derive(Subcommand, Debug)]
pub enum Mode {
    /// Process all pending inbox files, then exit. Cron mode.
    Once,
    /// Schema-check a single file. Exit 0 on success, 1 on failure.
    Validate {
        /// The file to validate against the configured schema.
        file: std::path::PathBuf,
    },
    /// Verify config + paths + schema are sane. Exit 0 on success, 1 on failure.
    Health,
}
