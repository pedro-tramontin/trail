//! `--once` mode: process all pending inbox files, append to plan file, move to processed.

use crate::config::CollectorConfig;
use anyhow::Result;

pub fn run(_cfg: &CollectorConfig) -> Result<i32> {
    // Fleshed in §1.8.
    anyhow::bail!("--once is not yet implemented (lands in §1.8)");
}
