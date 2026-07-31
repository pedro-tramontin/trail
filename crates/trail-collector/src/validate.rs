//! `--validate <file>` mode: schema-check a single file against the configured schema.

use crate::config::CollectorConfig;
use anyhow::Result;
use std::path::Path;

pub fn run(_cfg: &CollectorConfig, _file: &Path) -> Result<i32> {
    // Fleshed in §1.7.
    anyhow::bail!("--validate is not yet implemented (lands in §1.7)");
}
