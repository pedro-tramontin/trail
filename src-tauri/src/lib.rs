// `mod config;` lands in Phase 1 §1.2 (laptop config loader).
// `mod keyring;` lands in Phase 1 §1.3 (macOS Keychain keypair generator).
// `mod transport;` lands in Phase 1 §1.4 (SSH transport + IPC bindings).
// `mod commands;` lands in Phase 1 §1.5 (Tauri IPC bindings for the transport).
// `mod validate;` lands in Phase 1 §1.6 (client-side pre-push schema validation).
// They are added incrementally to the workspace below.

mod collectors;
mod commands;
mod config;
mod keyring;
mod transport;
mod validate;
// Frozen Phase 3 prompt template constants — see `src/prompts.rs` for the
// contract rationale.
pub mod prompts;
// Typed HTTP client for the local ollama server — see `src/ollama.rs`
// for the request shape + error type.
pub mod ollama;
// Phase 3 §3.2 — core summarize-day pipeline (loads raw/<date>/*.json,
// calls ollama, scrubs, validates the five `##` sections, writes the
// draft, returns a `SummarizeReceipt`).
pub mod summarizer;
// Phase 3 §3.2 shim — the real anonymizer regex scrubber lands in §3.3.
// Today this is a no-op pass-through so `summarizer::run` compiles +
// tests can exercise the scrubbing call site pre-§3.3.
pub mod anonymizer;
// Phase 3 §3.4 — the learner. Classifies user edits to the draft and
// maintains a `summary_bootstrap.json` file under the trail root so
// future summarizer runs see prior preferences as few-shot context.
// The `summarizer::run` signature takes a `bootstrap_path: &Path`
// argument and calls `learner::bootstrap_block` to render the
// in-prompt Markdown.
pub mod learner;
// Phase 3 §3.5 — the scheduler. Background tokio task that fires the
// summarizer at the configured `review_time`, updates the tray-icon
// badge with `drafts ready: N`, and records the last-fire timestamp
// in `SchedulerState`. This item only ships the standalone module
// + unit tests; wiring it into `run()` below is the coordinator's
// follow-up decision (so we keep the `mod` declaration `pub` for the
// tests + IPC visibility). Notifications via `notify-rust` are
// deferred to a follow-up item — the v1 surface is the tray-badge
// callback only.
pub mod scheduler;
// Phase 4 §4.1 — the logs backend. Three Tauri IPC commands
// (`list_logs`, `delete_log`, `get_raw_json`) over the day's raw
// collector files at `~/.trail/raw/<date>/*.json`. Kept `pub` so
// `commands.rs` can re-export the commands + tests can drive the
// module without an `AppHandle` or live filesystem.
pub mod logs;
// Phase 5 §5.1 — voice capture pipeline. This item only ships the
// `model_manager` sub-module (downloads + verifies the whisper GGML
// file). Audio capture (5-2), transcription (5-3), hotkey (5-4), and
// IPC (5-5) land in later items as siblings alongside `model_manager`.
pub mod voice;

use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust.")
}

#[tauri::command]
fn get_config(path: String) -> Result<config::Config, String> {
    config::load_config(std::path::Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
fn generate_ssh_key() -> Result<String, String> {
    keyring::generate_and_store().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_ssh_public_key() -> Result<Option<String>, String> {
    keyring::read_public_from_keychain().map_err(|e| e.to_string())
}

/// Build a `CollectorOrchestrator` from `config_path` + `collector_bin`,
/// loading the laptop `Config` so default-enable rules apply. Convenience
/// shared across the three IPC commands below; returns `String` for the
/// Tauri command shape.
async fn build_orchestrator(
    config_path: String,
    collector_bin: String,
) -> Result<collectors::CollectorOrchestrator, String> {
    let cfg = crate::config::load_config(std::path::Path::new(&config_path))
        .map_err(|e| e.to_string())?;
    Ok(collectors::CollectorOrchestrator::new(
        std::path::PathBuf::from(config_path),
        std::path::PathBuf::from(collector_bin),
        &cfg,
    ))
}

/// Tauri command: list every collector's current state (enabled, schedule,
/// last_run_at, last_exit_code, last_error). Returned in canonical order so
/// the Settings UI can render rows in a stable position.
#[tauri::command]
async fn list_collectors(
    config_path: String,
    collector_bin: String,
) -> Result<Vec<collectors::CollectorInfo>, String> {
    Ok(build_orchestrator(config_path, collector_bin)
        .await?
        .info()
        .await)
}

/// Tauri command: run one collector now (used by the "Run now" button on
/// each Settings row). Returns the collector's exit code (0 = success) and
/// records the result in the orchestrator's last-run state.
#[tauri::command]
async fn run_collector_now(
    source: String,
    config_path: String,
    collector_bin: String,
) -> Result<i32, String> {
    let orch = build_orchestrator(config_path, collector_bin).await?;
    orch.run_one(&source).await.map_err(|e| e.to_string())
}

/// Tauri command: flip a collector's enabled toggle. Returns the unit on
/// success; errors on unknown source so the UI surfaces a clear message.
#[tauri::command]
async fn set_collector_enabled(
    source: String,
    enabled: bool,
    config_path: String,
    collector_bin: String,
) -> Result<(), String> {
    build_orchestrator(config_path, collector_bin)
        .await?
        .set_enabled(&source, enabled)
        .await
        .map_err(|e| e.to_string())
}

/// Resolve the per-app config + collector binary paths. On a normal
/// Tauri launch these come from `app.path()` (the platform-correct
/// per-user config dir for the bundled `.app` / `.msi` / `.deb`). The
/// fallback `~/.<config>` is for `cargo test` and headless dev runs
/// where `tauri::generate_context!()` isn't available.
fn resolve_paths(
    app: Option<&tauri::AppHandle>,
) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
    let config_path = if let Some(app) = app {
        let dir = app.path().app_config_dir()?;
        dir.join("config.json")
    } else {
        PathBuf::from(std::env::var("HOME").map_err(|_| "HOME not set")?)
            .join(".trail")
            .join("config.json")
    };
    // The collector binary is the bundled `trail-collector` that the
    // Tauri build script + `tauri.conf.json` place next to the main
    // executable. In test/dev we let `COLLECTOR_BIN` override (defaults
    // to the same `trail-collector` name on $PATH).
    let collector_bin = if let Ok(p) = std::env::var("COLLECTOR_BIN") {
        PathBuf::from(p)
    } else {
        PathBuf::from("trail-collector")
    };
    Ok((config_path, collector_bin))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // Build the orchestrator once at launch, hand it to the
            // Tauri-managed state so IPC commands see the same
            // `last_run_at` / `last_exit_code` / `last_error` the
            // scheduler writes, then spawn the scheduler task.
            let (config_path, collector_bin) =
                resolve_paths(Some(app.handle())).map_err(|e| -> Box<dyn std::error::Error> {
                    format!("resolving config paths: {e}").into()
                })?;
            // Phase 5 §5.6 — register the shared `CaptureState` so the
            // `voice_abort` IPC command can clear the in-memory samples
            // buffer and `.abort()` the consumer task. Wrapped in an
            // `Arc` because Tauri hands the State back by value and
            // `CaptureState` is shared between the cpal-callback
            // thread, the consumer task, and the abort handler.
            app.manage(std::sync::Arc::new(
                crate::voice::capture::CaptureState::new(),
            ));
            let cfg =
                config::load_config(&config_path).map_err(|e| -> Box<dyn std::error::Error> {
                    format!("loading config from {}: {e}", config_path.display()).into()
                })?;
            let orch = Arc::new(collectors::CollectorOrchestrator::new(
                config_path,
                collector_bin,
                &cfg,
            ));
            let orch_for_sched = orch.clone();
            let sched_task = tokio::spawn(async move {
                match orch_for_sched.start_scheduler().await {
                    Ok(mut sched) => {
                        if let Err(e) = sched.start().await {
                            tracing::error!(error = %e, "scheduler.start() failed");
                            return;
                        }
                        tracing::info!("collector scheduler started");
                        // Park until the runtime shuts down. Tauri 2
                        // aborts spawned tasks on exit; we also call
                        // `scheduler.shutdown()` in a Drop below via
                        // a graceful path on the tokio runtime's
                        // teardown. In practice the runtime drop
                        // releases all spawned tasks and the
                        // scheduler's internal channels close.
                        std::future::pending::<()>().await;
                        if let Err(e) = sched.shutdown().await {
                            tracing::warn!(error = %e, "scheduler.shutdown() failed");
                        }
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "start_scheduler() failed");
                    }
                }
            });
            // Stash the JoinHandle so a future shutdown signal can
            // await it (Tauri 2 doesn't yet expose a clean "app is
            // exiting" hook in the setup closure). For v1 the
            // `pending().await` above is fine because the orchestrator
            // doesn't need to run after the user quits the menu-bar
            // app.
            app.manage(orch);
            app.manage(Arc::new(sched_task));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            get_config,
            generate_ssh_key,
            get_ssh_public_key,
            commands::health_check_transport,
            commands::push_to_vps,
            commands::validate_day_summary,
            commands::summarize_day,
            commands::record_review_diff,
            commands::list_logs,
            commands::delete_log,
            commands::get_raw_json,
            // Phase 5 §5.6 — voice abort IPC command. Clears the
            // shared samples buffer, aborts the consumer task, and
            // removes any partial WAV + JSON files.
            commands::voice_abort,
            list_collectors,
            run_collector_now,
            set_collector_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running trail");
}
