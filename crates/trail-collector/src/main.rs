use anyhow::{Context, Result};
use clap::Parser;
use trail_collector::{collect, collectors, config, health, Cli, Mode};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let exit_code = match cli.mode {
        Mode::Health => health::run(&load_vps(&cli.config)?),
        Mode::Validate { file } => {
            trail_collector::validate::run(&load_vps(&cli.config)?, &file).context("--validate")?
        }
        Mode::Once => trail_collector::once::run(&load_vps(&cli.config)?).context("--once")?,
        Mode::Collect {
            source,
            laptop_config,
        } => {
            let mut lc = load_laptop(&laptop_config)?;
            lc.source = source;
            collect::run(&lc)?
        }
    };
    std::process::exit(exit_code);
}

fn load_vps(path: &std::path::Path) -> Result<config::CollectorConfig> {
    config::load(path).with_context(|| format!("loading VPS config from {}", path.display()))
}

fn load_laptop(path: &std::path::Path) -> Result<collectors::CollectorLaptopConfig> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading laptop config {}", path.display()))?;
    let lc: collectors::CollectorLaptopConfig = serde_json::from_str(&text)
        .with_context(|| format!("parsing laptop config {}", path.display()))?;
    Ok(lc)
}
