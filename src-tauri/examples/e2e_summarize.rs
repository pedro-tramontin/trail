//! Phase 3 §3.6 e2e example — runs the summarizer pipeline against a
//! mock ollama (via env var OLLAMA_BASE_URL) + a temp TRAIL_HOME.
//!
//! Usage:
//!   TRAIL_HOME=/tmp/trail-e2e \
//!   OLLAMA_BASE_URL=http://127.0.0.1:11434 \
//!   cargo run -p trail --example e2e_summarize -- --date 2026-07-29

use std::path::PathBuf;

use trail_lib::ollama::OllamaClient;
use trail_lib::summarizer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let date = args
        .iter()
        .position(|a| a == "--date")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_else(|| {
            let now = chrono::Utc::now();
            now.format("%Y-%m-%d").to_string()
        });
    let trail_home: PathBuf = std::env::var("TRAIL_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".trail")
        });
    let endpoint =
        std::env::var("OLLAMA_BASE_URL").unwrap_or_else(|_| "http://localhost:11434".to_string());
    let raw_root = trail_home.join("raw");
    let drafts_dir = trail_home.join("drafts");
    let bootstrap_path = trail_home.join("summary_bootstrap.json");
    let client = OllamaClient::new(endpoint);
    println!(
        "e2e_summarize: date={date} trail_home={} endpoint={}",
        trail_home.display(),
        client.endpoint
    );
    let receipt = summarizer::run(
        &raw_root,
        &drafts_dir,
        &bootstrap_path,
        &date,
        "llama3",
        "aggressive",
        &client,
    )
    .await?;
    println!("e2e_summarize: receipt={receipt:?}");
    Ok(())
}
