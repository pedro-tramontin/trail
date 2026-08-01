//! Phase 3 §3.6 e2e example — records a user edit as a learner event.
//!
//! Usage:
//!   TRAIL_HOME=/tmp/trail-e2e \
//!   cargo run -p trail --example e2e_learn -- --before "old text" --after "new text"

use std::path::PathBuf;

use trail_lib::learner::{classify, record_event};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let before = args
        .iter()
        .position(|a| a == "--before")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let after = args
        .iter()
        .position(|a| a == "--after")
        .and_then(|i| args.get(i + 1))
        .cloned()
        .unwrap_or_default();
    let trail_home: PathBuf = std::env::var("TRAIL_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".trail")
        });
    let bootstrap_path = trail_home.join("summary_bootstrap.json");
    let kind = classify(&before, &after);
    println!("e2e_learn: classify({before:?}, {after:?}) = {kind:?}");
    let bootstrap = record_event(&bootstrap_path, kind, &before, &after)?;
    println!(
        "e2e_learn: bootstrap now has {} rules",
        bootstrap.rules.len()
    );
    Ok(())
}
