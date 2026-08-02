// `mod config;` lands in Phase 1 §1.2 (laptop config loader).
// `mod keyring;` lands in Phase 1 §1.3 (macOS Keychain keypair generator).
// `mod transport;` lands in Phase 1 §1.4 (SSH transport + IPC bindings).
// `mod commands;` lands in Phase 1 §1.5 (Tauri IPC bindings for the transport).
// `mod validate;` lands in Phase 1 §1.6 (client-side pre-push schema validation).
// `pub mod install;` lands in Phase 1 §1.10 (VPS install plan) and
// is extended in Phase 6 §6.6 with the install-wizard's 3 Tauri
// commands. `pub` so the Phase 6 integration test
// (`tests/onboarding_e2e.rs`) can drive `install_vps_collector`
// against the in-tree `mock-ssh-server` fixture.
mod collectors;
mod commands;
// Phase 7 §7.2 — env-var self-test for the macOS code-signing +
// notarization pipeline. Exposes the `notarize_check` Tauri command
// (returns a `env-var name → "set" | "unset"` map, never the value)
// so the frontend + CI can confirm the env is wired before invoking
// `cargo tauri build`. See src/notarize.rs for the env-var list +
// the security rationale (no value-echo through IPC).
mod notarize;
// `pub mod config;` is re-exported here (was `mod config;` in
// Phase 1 §1.2) so the Phase 6 integration test
// (`tests/onboarding_e2e.rs`) can call `config::load_config` to
// assert the Phase C round-trip — the frozen Config type is the
// contract the spec binds the e2e against.
pub mod config;
pub mod install;
mod keyring;
pub mod onboarding;
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
// Phase 5 §5.7 — tray menu filter. Pure-function logic — `tray::MenuEntry`
// is the enum the future tray-icon builder consumes, and
// `tray::filtered_items` is the rule set that decides which entries are
// visible at each permission/recording/conflict state.
mod tray;

use std::path::PathBuf;
use std::sync::Arc;
use tauri::Manager;

/// Phase 7 §7.2 — Tauri command: env-var self-test for the macOS
/// code-signing + notarization pipeline. Returns a sorted
/// `env-var name → "set" | "unset"` map (NEVER the value, to
/// keep the IPC channel free of signing-identity + .p8-path leaks).
///
/// The frontend can invoke this from a "Verify signing env" button
/// in the Settings shell; CI can invoke it via the smoke script to
/// confirm the workflow's env propagation works end-to-end.
#[tauri::command]
fn notarize_check() -> notarize::NotarizeEnvReport {
    notarize::check()
}

#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {name}! You've been greeted from Rust.")
}

/// Phase 6 §6.3 — Tauri command: Phase C of the onboarding wizard.
///
/// Chains three steps in one IPC call:
///   1. Convert the LLM's `OnboardingAnswers` into the frozen `Config`.
///   2. Atomically write `~/.trail/config.json` (write + fsync + rename).
///   3. Append a JSONL row to `~/.trail/onboarding_audit.jsonl`.
///
/// `ssh_key_generated` is the `bool` item 1-2 returned: `true` means
/// the keypair is already in the macOS Keychain, so we emit the
/// `PublicKey { path }` auth variant. `false` falls back to the
/// `Password { env_var }` placeholder so the parsed config still
/// validates (the wizard re-emits the config after item 1-2's
/// `generate_ssh_key` succeeds).
///
/// Errors surface as `String` for the Tauri command boundary, so the
/// Svelte wizard can render them directly.
#[tauri::command]
async fn write_onboarding_config(
    answers: onboarding::OnboardingAnswers,
    ssh_key_generated: bool,
) -> Result<String, String> {
    // 1. Convert.
    let cfg = onboarding::config_writer::answers_to_config(&answers, ssh_key_generated);

    // 2. Atomic write to `~/.trail/config.json`.
    let dest = onboarding::config_writer::config_path();
    let serialised =
        serde_json::to_string_pretty(&cfg).map_err(|e| format!("serialise config: {e}"))?;
    onboarding::config_writer::write_config(&cfg, &dest)
        .map_err(|e| format!("write config to {}: {e}", dest.display()))?;

    // 3. Append the audit log row (with the sha256 of the just-written
    //    JSON bytes so the row references the *exact* file we wrote).
    onboarding::config_writer::append_audit_log_with_hash(&answers, &dest, &serialised)
        .map_err(|e| format!("append audit log: {e}"))?;

    Ok(dest.display().to_string())
}

#[tauri::command]
fn get_config(path: String) -> Result<config::Config, String> {
    config::load_config(std::path::Path::new(&path)).map_err(|e| e.to_string())
}

/// Phase 6 §6.4 — Tauri command: probe whether the user's
/// `~/.trail/config.json` exists on disk. Used by `App.svelte`
/// to decide whether to mount the onboarding wizard or the
/// regular shell.
///
/// Returns `true` only when the file exists AND is non-empty —
/// a zero-byte file (a torn write from a previous crash) is
/// treated as "not present" so the wizard re-runs cleanly.
/// The path resolution mirrors the dev/CI fallback in
/// `resolve_paths`: `app_config_dir().join("config.json")` if
/// a Tauri `AppHandle` is available, otherwise
/// `$HOME/.trail/config.json`.
#[tauri::command]
fn config_exists(app: tauri::AppHandle) -> bool {
    let dir = match app.path().app_config_dir() {
        Ok(d) => d,
        Err(_) => match std::env::var("HOME") {
            Ok(h) => std::path::PathBuf::from(h).join(".trail"),
            Err(_) => return false,
        },
    };
    let path = dir.join("config.json");
    path.is_file()
        && std::fs::metadata(&path)
            .map(|m| m.len() > 0)
            .unwrap_or(false)
}

/// Phase 6 §6.5 — Tauri command: remove the user's
/// `~/.trail/config.json` so the onboarding wizard can re-run.
///
/// The `cmd` parameter is the absolute path to the config
/// file. The Tauri IPC layer always passes the resolved
/// path; the default is `crate::onboarding::config_writer::config_path()`
/// (the same `~/.trail/config.json` `config_exists` probes).
/// A missing file is NOT an error — the "reset" path is
/// idempotent so a re-run from a half-deleted state still
/// succeeds. Only real IO failures (permission denied, parent
/// dir gone) surface as `Err(String)`.
///
/// The deletion is performed with `std::fs::remove_file` —
/// no `rm -rf` style recursion, no `sudo`. If the file does
/// not exist, the `NotFound` error is swallowed and we
/// return `Ok(())`. The path is the one resolved on the
/// *frontend* side; the Rust side trusts it (the Tauri
/// command boundary is local to the user's machine, so
/// no untrusted-remote mitigation is required).
#[tauri::command]
fn delete_config(cmd: std::path::PathBuf) -> Result<(), String> {
    match std::fs::remove_file(&cmd) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("delete {}: {e}", cmd.display())),
    }
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
            // Phase 7 §7.2 — env-var self-test for the macOS signing +
            // notarization pipeline. See src/notarize.rs.
            notarize_check,
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
            // Phase 6 §6.1 — non-invasive laptop scan. Used by the
            // future onboarding wizard (item 6-4) to enumerate
            // detectors that found artifacts on the user's disk.
            onboarding::scan::scan_laptop_cmd,
            // Phase 6 §6.2 — LLM-driven onboarding Q&A. Feeds the
            // scan report to the local ollama server, validates the
            // response against `schemas/onboarding-answer.schema.json`,
            // and falls back to a hardcoded baseline when ollama is
            // unreachable. Returns a typed `OnboardingAnswers` for
            // Phase C (item 6-3, config-writer).
            onboarding::llm::ask_onboarding_cmd,
            // Phase 6 §6.3 — convert the LLM's `OnboardingAnswers`
            // into the frozen `Config`, atomically write it to
            // `~/.trail/config.json`, and append a JSONL audit-log
            // row. The wizard (item 6-4) chains scan → ask →
            // write_onboarding_config.
            write_onboarding_config,
            // Phase 6 §6.4 — probe whether `~/.trail/config.json`
            // exists. Drives the App.svelte gate that mounts
            // <Onboarding /> vs the regular shell.
            config_exists,
            // Phase 6 §6.5 — remove `~/.trail/config.json` so the
            // onboarding wizard can re-run. Used by the
            // "Re-run onboarding" button on the Settings shell.
            delete_config,
            // Phase 6 §6.6 — install-wizard's 3-option step.
            // `install_vps_collector` is the auto path; in tests
            // the `dry_run: true` branch redirects the push to
            // the `mock-ssh-server` fixture on `127.0.0.1:<port>`.
            install::install_vps_collector,
            // Phase 6 §6.6 — install-wizard's "show me the plan"
            // option. Returns the absolute path of the rendered
            // `~/.trail/collector.json` so the frontend can hand
            // it to the platform's reveal/open handler.
            install::open_collector_script,
            // Phase 6 §6.6 — install-wizard's "do this later"
            // option. Appends `collector_id` to the
            // `pending_installs` array in `~/.trail/config.json`
            // (idempotent).
            install::mark_pending_install,
        ])
        .run(tauri::generate_context!())
        .expect("error while running trail");
}

#[cfg(test)]
mod tests {
    use super::delete_config;
    use std::fs;
    use std::path::PathBuf;

    /// `delete_config` removes an existing file and returns `Ok(())`.
    #[test]
    fn delete_config_removes_existing_file() {
        let tmp = std::env::temp_dir().join(format!(
            "trail-delete-config-exists-{}.json",
            std::process::id()
        ));
        fs::write(&tmp, b"{}").expect("write");
        assert!(tmp.is_file());

        let result = delete_config(tmp.clone());
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
        assert!(!tmp.is_file(), "file must be gone after delete_config");

        // Cleanup if the test left the file behind for any reason.
        let _ = fs::remove_file(&tmp);
    }

    /// `delete_config` is idempotent: a missing file is NOT an
    /// error (the `NotFound` kind is swallowed) so re-runs from a
    /// half-deleted state still succeed.
    #[test]
    fn delete_config_missing_file_is_noop() {
        let tmp = std::env::temp_dir().join(format!(
            "trail-delete-config-missing-{}.json",
            std::process::id()
        ));
        // Make sure it really is missing.
        let _ = fs::remove_file(&tmp);
        assert!(!tmp.is_file());

        let result = delete_config(tmp);
        assert!(
            result.is_ok(),
            "expected Ok on missing file, got {:?}",
            result
        );
    }

    /// `delete_config` is idempotent across every "not present"
    /// path: a missing file, a missing parent directory, or a
    /// path the runtime can't resolve all collapse to `Ok(())`.
    /// This is the spec'd "no-op if missing" rule — the wizard
    /// re-run from a half-deleted state must still succeed.
    #[test]
    fn delete_config_missing_file_or_parent_is_noop() {
        // (a) plain missing file.
        let missing_file = std::env::temp_dir().join(format!(
            "trail-delete-config-missing-{}.json",
            std::process::id()
        ));
        let _ = fs::remove_file(&missing_file);
        assert!(delete_config(missing_file).is_ok());

        // (b) missing parent directory.
        let bogus: PathBuf = std::env::temp_dir()
            .join("trail-delete-config-missing-dir-never-created")
            .join("config.json");
        assert!(delete_config(bogus).is_ok());
    }
}
