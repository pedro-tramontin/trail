use anyhow::{Context, Result};
use clap::Parser;
use trail_collector::{config, health, Cli, Mode};

fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = config::load(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    let exit_code = match cli.mode {
        Mode::Health => health::run(&cfg),
        Mode::Validate { file } => {
            trail_collector::validate::run(&cfg, &file).context("--validate")?
        }
        Mode::Once => trail_collector::once::run(&cfg).context("--once")?,
    };
    std::process::exit(exit_code);
}
