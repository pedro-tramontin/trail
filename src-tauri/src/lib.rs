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
// Phase 9 §9.1 — bridge module that hosts the `start_collectors`
// Tauri command. Kept separate from the crate root so tauri's
// macro_export re-export on `pub` commands doesn't collide at
// `__cmd__start_collectors` (see src/setup_bridge.rs doc comment).
// `pub` since Phase 9 §9.3 so the §9.3 integration test in
// `tests/headless_launch.rs` can call
// `setup_bridge::window_descriptor_for` and
// `setup_bridge::build_initial_window` to verify the
// `ConfigState` → window-label logic without a live Tauri runtime
// (Tauri 2.11.5's `mock_builder().build()` shim does NOT run the
// setup closure synchronously — see §9.1 D2 in state.md).
pub mod setup_bridge;
// Phase 9 §9.2 — bridge module that hosts the `show_main_window`
// Tauri command (used by the tray-icon "Show Trail" menu item +
// the future wizard "Open settings" button). Same
// macro_export-collision rationale as `setup_bridge` above — see
// src/window_bridge.rs doc comment. `pub` so the §9.2 integration
// test in `tests/headless_launch.rs` can assert the menu
// descriptor content (`MAIN_TRAY_MENU_ITEMS`) without a live
// Tauri runtime.
pub mod window_bridge;
// Phase 7 §7.5 — demo mode first-run experience. The
// `activate_if_requested` function decides whether to boot with
// fixture data + a yellow banner, gated on (--demo flag) AND
// (no `~/.trail/config.json` on disk).
mod demo;
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
// Phase 9 §9.2 — tray icon + menu builder (replaces the §5.7
// dead-code scaffold). The Phase 5.7 `tray::MenuEntry` enum +
// `filtered_items` rule set were never wired into a live
// `TrayIconBuilder` call — §5.9 was deferred past Phase 5, so the
// items sat behind `#[allow(dead_code)]` until §9.2 deleted the
// whole module. The "Show Trail" + "Quit Trail" items the binary
// actually surfaces now are built imperatively via
// `tauri::menu::Menu` + `tauri::tray::TrayIconBuilder` in the
// `run()` setup closure, which is the Tauri 2 way.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tauri::Manager;

/// Phase 9 §9.1 — runtime state for the config lifecycle.
///
/// `Ready` means config exists; the collector orchestrator + scheduler
/// are already running and managed by Tauri. `AwaitingOnboarding` means
/// no config has been written yet — the setup closure deliberately did
/// not build the orchestrator; the Svelte side's first-run wizard calls
/// `start_collectors` after writing the config to bring the collectors
/// up lazily. This replaces the old "panic if config missing" behavior
/// with a clean state machine the frontend can drive.
#[derive(Debug, Clone)]
pub enum ConfigState {
    /// Config on disk + collectors running. Carries the parsed config
    /// so future IPC commands can read fields without re-loading the
    /// file.
    /// Real config has been written to disk AND the orchestrator +
    /// scheduler are running. The `Config` is boxed to keep the
    /// enum small (clippy::large_enum_variant; Config is ~600
    /// bytes, vs. AwaitingOnboarding's 0 bytes — without
    /// `Box`, every match on `ConfigState` would carry a
    /// 600-byte discriminant that lives on the stack).
    Ready(Box<crate::config::Config>),
    /// No config on disk yet. The wizard will write one, then call
    /// `start_collectors` to flip into `Ready`.
    AwaitingOnboarding,
}

/// Phase 9 §9.1 — lazy-init the collector orchestrator + scheduler.
///
/// Called by the Svelte frontend at wizard `StepFinish` time, after
/// `write_onboarding_config` has succeeded. Returns `Err` if the config
/// is missing or fails to parse — a buggy frontend firing this before
/// the wizard finishes gets a clean error rather than an orchestrator
/// crash or a silent no-op.
///
/// Returns the built orchestrator + the spawned scheduler `JoinHandle`
/// so the caller (the Tauri IPC wrapper, in production) can hand them
/// to `app.manage`. Unit tests just discard the tuple. `pub` so the
/// `tests/headless_launch.rs` integration test (which mirrors the
/// real setup closure's logic) can drive it from outside the crate
/// boundary.
pub fn start_collectors_inner(
    config_path: &Path,
) -> Result<
    (
        Arc<collectors::CollectorOrchestrator>,
        tokio::task::JoinHandle<()>,
    ),
    String,
> {
    let cfg = config::load_config(config_path)
        .map_err(|e| format!("loading config from {}: {e}", config_path.display()))?;
    let collector_bin = if let Ok(p) = std::env::var("COLLECTOR_BIN") {
        PathBuf::from(p)
    } else {
        PathBuf::from("trail-collector")
    };
    let orch = Arc::new(collectors::CollectorOrchestrator::new(
        config_path.to_path_buf(),
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
                // Park until the runtime shuts down; Tauri 2 aborts
                // spawned tasks on exit so the scheduler's internal
                // channels close cleanly at teardown.
                std::future::pending::<()>().await;
                let _ = sched.shutdown().await;
            }
            Err(e) => {
                tracing::error!(error = %e, "start_scheduler() failed");
            }
        }
    });
    Ok((orch, sched_task))
}

/// Phase 9 §9.1 — Tauri command proxy at the crate root.
///
/// The actual `#[tauri::command]` definition lives in the
/// `setup_bridge` sub-module (see `src/setup_bridge.rs`); keeping
/// it out of the crate root avoids the tauri-macros 2.6.3
/// `#[macro_export]` collision where both the macro_rules!
/// definition and the visibility-scoped `use` re-export would
/// otherwise land at the same crate-root path (`__cmd__start_collectors`).
/// This proxy exists purely so the `generate_handler!` macro
/// can list `start_collectors` at the lib.rs level and the IPC
/// contract name stays a flat `start_collectors` (not
/// `setup_bridge::start_collectors`).
fn start_collectors(app: tauri::AppHandle) -> Result<(), String> {
    setup_bridge::start_collectors(app)
}

/// Phase 9 §9.2 — Tauri command proxy at the crate root.
///
/// Same pattern as `start_collectors` above: the real
/// `#[tauri::command]` lives in `window_bridge` to keep the
/// macro_export re-export out of the crate root. The
/// tray-icon "Show Trail" menu item and the future wizard
/// "Open settings" button both invoke `show_main_window` over
/// the IPC channel.
fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    window_bridge::show_main_window(app)
}

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

/// Phase 7 §7.5 — Tauri command: returns the current demo state
/// so the Svelte `<DemoBanner />` + `<Review />` can render
/// fixture data + the yellow banner. Reads the same
/// `TRAIL_DEMO` env var the binary set on `--demo`.
#[tauri::command]
fn demo_status() -> Option<demo::DemoState> {
    let args = demo::Args {
        demo: std::env::var("TRAIL_DEMO")
            .map(|v| v == "1")
            .unwrap_or(false),
    };
    demo::activate_if_requested(&args).ok().flatten()
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
    app: tauri::AppHandle,
    answers: onboarding::OnboardingAnswers,
    ssh_key_generated: bool,
) -> Result<String, String> {
    // 1. Convert.
    let cfg = onboarding::config_writer::answers_to_config(&answers, ssh_key_generated);

    // 2. Resolve the destination via the platform-correct per-app
    //    config dir (`app_config_dir()`) so the path matches the
    //    reader (`start_collectors`) and the wizard-gate
    //    (`config_exists`). Pre-PR bug: we used
    //    `config_writer::config_path()` which always returns
    //    `$HOME/.trail/config.json` — that's correct for `cargo
    //    test` / headless dev but WRONG for a real macOS install,
    //    where `app_config_dir()` returns
    //    `~/Library/Application Support/com.<bundle-id>/config.json`.
    //    The wizard said "Config written to ~/.trail/config.json"
    //    while the rest of the app looked in
    //    `~/Library/Application Support/.../config.json`, so the
    //    user could `cat` the displayed path and find no file,
    //    and the wizard gate never advanced.
    let dest = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("resolve app_config_dir: {e}"))?
        .join("config.json");
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
    // Phase 7 §7.5 — decide demo mode BEFORE the Tauri builder so
    // we can `app.manage(demo_state)` and have it ready when the
    // Svelte side's first `invoke('demo_status')` fires. The flag
    // itself is propagated from `main.rs` via the `TRAIL_DEMO`
    // env var (Tauri's `lib::run()` doesn't take args).
    let args = demo::Args {
        demo: std::env::var("TRAIL_DEMO")
            .map(|v| v == "1")
            .unwrap_or(false),
    };
    let demo_state = demo::activate_if_requested(&args).ok().flatten();

    tauri::Builder::default()
        .setup(move |app| {
            // Phase 7 §7.5 — share the demo state with the
            // frontend. When `demo_state` is `None` the Svelte side
            // sees a missing state and renders no banner.
            if let Some(ref state) = demo_state {
                app.manage(state.clone());
            }
            // Build the orchestrator once at launch, hand it to the
            // Tauri-managed state so IPC commands see the same
            // `last_run_at` / `last_exit_code` / `last_error` the
            // scheduler writes, then spawn the scheduler task.
            // Resolve the per-app config + collector binary paths.
            // `collector_bin` is only consumed by `start_collectors_inner`
            // (which re-derives it from `COLLECTOR_BIN`/`trail-collector`).
            // We bind it here so the resolution site stays in one place and
            // the post-load-config error path can still show the resolved
            // path in its message.
            let (config_path, _collector_bin) =
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
            // Phase 9 §9.1 — skip `load_config` when missing; defer
            // the collector orchestrator until `start_collectors` IPC
            // fires. Old behavior (lines 357-365 pre-§9.1): call
            // `load_config` unconditionally, propagate `NotFound` via
            // `?`, crash the Tauri runtime before any UI is built.
            // New behavior:
            //   - `Ok(cfg)`      → build orchestrator + scheduler
            //                      (Phase 2-7 behavior, unchanged).
            //   - `Err(NotFound)`→ register an `AwaitingOnboarding`
            //                      sentinel; the wizard will write the
            //                      config then call `start_collectors`.
            //   - `Err(other)`   → propagate (real IO / JSON failure).
            let config_state = match config::load_config(&config_path) {
                Ok(cfg) => {
                    // Phase 9 §9.1 — reuse `start_collectors_inner` so the
                    // "existing-config" path and the "wizard just wrote a
                    // config" path share the same orchestrator-build +
                    // scheduler-spawn sequence.
                    let (orch, sched_task) = start_collectors_inner(&config_path)?;
                    app.manage(orch);
                    app.manage(Arc::new(sched_task));
                    ConfigState::Ready(Box::new(cfg))
                }
                Err(config::ConfigError::NotFound(_)) => {
                    // First-launch path: register a sentinel. The frontend's
                    // `start_collectors` IPC will build the orchestrator
                    // after the wizard writes the config.
                    tracing::info!(
                        "No config at {}; running in pre-onboarding mode",
                        config_path.display()
                    );
                    ConfigState::AwaitingOnboarding
                }
                Err(e) => {
                    // Real error (IO failure, JSON parse failure, etc.).
                    // Surface it so the user sees a clear panic dialog
                    // instead of the silent-exit + `Ok` state we used to
                    // hit on `NotFound`.
                    return Err(
                        format!("loading config from {}: {e}", config_path.display()).into(),
                    );
                }
            };
            app.manage(config_state);
            // `config_path` has been moved into `start_collectors_inner`
            // (via `to_path_buf()`) when the `Ok` arm ran, or surfaced in
            // the error message when the `Err(other)` arm ran. We don't
            // need it again here — the IPC `start_collectors` command
            // resolves it fresh from the `AppHandle`. `_collector_bin`
            // is dropped automatically at the closure's end (it was
            // only ever a let-binding here).

            // Phase 9 §9.3 — open the initial webview window
            // imperatively. When a config already exists on disk
            // (the `Ready` arm ran), open the `main` shell; when
            // there's no config yet (the `AwaitingOnboarding` arm
            // ran), open the `onboarding` wizard. This is the
            // counterpart to the `tauri.conf.json` `"windows":
            // [{ "label": "main", "visible": false }]` fallback —
            // the fallback registers the window with the Tauri
            // runtime so `WebviewWindowBuilder::new` doesn't
            // panic on a duplicate label, and `visible: false`
            // keeps the empty default window from flashing before
            // the setup closure runs. The setup closure then
            // shows the appropriate window per the `ConfigState`.
            //
            // Placed BEFORE the §9.2 tray-icon build so the main
            // window exists when the tray-icon left-click handler
            // fires (otherwise the "show main" handler would be
            // a no-op for the first half-second after launch).
            //
            // The actual builder call lives in `setup_bridge::build_initial_window`
            // so the §9.3 integration test
            // (`tests/headless_launch.rs::headless_launch_opens_onboarding_window_when_no_config`)
            // can assert the `ConfigState` → `WindowDescriptor`
            // logic via the `window_descriptor_for` helper
            // without dragging in the Tauri 2.11.5 runtime (whose
            // `mock_builder().build()` shim does NOT run the
            // setup closure synchronously — see the §9.1 D2 / §9.2
            // lessons in state.md).
            {
                let state = app.state::<ConfigState>();
                // The setup closure receives `&mut tauri::App`;
                // `setup_bridge::build_initial_window` takes
                // `&tauri::AppHandle<R>` (the trait the
                // `WebviewWindowBuilder::new` constructor wants).
                // `App::handle()` returns the `&AppHandle`
                // directly (same pattern `resolve_paths` uses
                // a few lines up — see line 488).
                setup_bridge::build_initial_window(app.handle(), &state)?;
            }
            tracing::info!("initial webview window built");

            // Phase 9 §9.2 — build the tray icon imperatively. The
            // Tauri 2 way: a `tauri::menu::Menu` of "Show Trail" +
            // "Quit Trail" items + a `tauri::tray::TrayIconBuilder`
            // registered as `"main-tray"`. The menu and click
            // handlers live here in the setup closure (the binary
            // is a menu-bar app, so this is the only "main" UI on
            // macOS).
            //
            // Why imperative (not just the `tauri.conf.json`
            // `trayIcon` declarative block — which IS still there
            // as the Tauri-runtime-level default): the spec
            // requires (a) the menu items as a typed `Menu` and
            // (b) the left-click + right-click handlers wired in
            // the same closure that owns the rest of the app's
            // setup. The declarative block can't host closures;
            // the imperative builder can.
            //
            // The menu items are sourced from the
            // `MAIN_TRAY_MENU_ITEMS` static in `window_bridge` so
            // the `main_tray_menu_*` unit tests can assert the
            // menu content without a live Tauri runtime (see
            // `src/window_bridge.rs` for the rationale).
            //
            // Threading note: the setup closure runs on the main
            // thread *before* the event loop starts pumping user
            // messages. Tauri 2's `TrayIconBuilder::build` calls
            // `app_handle.run_on_main_thread(...)` internally and
            // then blocks on a `mpsc::channel().recv()` for the
            // result. `tauri-runtime-wry`'s `send_user_message`
            // short-circuits to inline execution when called from
            // the main thread (see
            // `tauri-runtime-wry-2.11.4/src/lib.rs:230-247`), so
            // the channel send happens synchronously and the
            // `recv` succeeds immediately. No deadlock.
            use tauri::menu::Menu;
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder};
            // Build the `MenuItem` handles from the static
            // descriptor (keeps the unit-testable surface in
            // `window_bridge` rather than re-declared here).
            let menu_items: Vec<tauri::menu::MenuItem<tauri::Wry>> =
                window_bridge::MAIN_TRAY_MENU_ITEMS
                    .iter()
                    .map(|entry| {
                        tauri::menu::MenuItem::with_id(
                            app,
                            entry.id,
                            entry.label,
                            true,
                            None::<&str>,
                        )
                    })
                    .collect::<tauri::Result<Vec<_>>>()?;
            let menu_refs: Vec<&dyn tauri::menu::IsMenuItem<tauri::Wry>> = menu_items
                .iter()
                .map(|m| m as &dyn tauri::menu::IsMenuItem<tauri::Wry>)
                .collect();
            let tray_menu = Menu::with_items(app, &menu_refs)?;
            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(
                    app.default_window_icon()
                        .cloned()
                        .ok_or("default window icon missing")?,
                )
                .icon_as_template(true) // macOS menu-bar tinting (per tauri.conf.json)
                .tooltip("Trail — passive workday capture")
                .menu(&tray_menu)
                .show_menu_on_left_click(false) // left click → show main window (macOS-native)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                    "quit" => {
                        app.exit(0);
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    // Left-click on the tray icon shows the main
                    // window (matches macOS-native menu-bar app
                    // behavior). Other click types are ignored
                    // (right-click is the menu).
                    if let tauri::tray::TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(win) = app.get_webview_window("main") {
                            let _ = win.show();
                            let _ = win.set_focus();
                        }
                    }
                })
                .build(app)?;
            tracing::info!("tray icon built (id=main-tray)");
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
            demo_status,
            commands::health_check_transport,
            commands::push_to_vps,
            // Wizard "Test connection" button — probes a
            // not-yet-persisted SSH connection (no config on disk
            // yet). Distinct from `health_check_transport` which
            // reads the on-disk config.
            commands::test_ssh_connection,
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
            // Phase 5 §5.5 — voice capture IPC commands. Wired here
            // (§17-8) so the platform-agnostic cpal + whisper
            // pipeline is reachable from the menu-bar UI's
            // push-to-talk + "Test microphone" buttons on all 3 OSes.
            commands::voice_start,
            commands::voice_stop,
            // §17-5 — per-OS microphone permission IPC commands.
            // The wizard's "Test microphone" button + the
            // Settings permission row read the current OS-level
            // permission state via `check_mic_permission_cmd`,
            // trigger the OS prompt via
            // `request_mic_permission_cmd`, and resolve the
            // per-OS deep-link URL via
            // `mic_permission_deep_link_url_cmd` so the
            // frontend can hand it to `tauri-plugin-opener` on
            // the denied callout.
            commands::check_mic_permission_cmd,
            commands::request_mic_permission_cmd,
            commands::mic_permission_deep_link_url_cmd,
            // §X-4 — per-OS calendar permission deep-link IPC
            // command. The wizard's EventKit hint (see
            // `StepAsk.svelte`'s calendar row + the calendar
            // permission denied callout) uses this to resolve
            // the per-OS URL the frontend hands to the system
            // browser handler when the user clicks "Open
            // Calendar Settings". On Linux + `de == None`
            // (DE can't be detected) the command returns a
            // structured error string the frontend uses to
            // render the "open Settings → Privacy → Calendar
            // manually" labeled fallback. See
            // `commands::calendar_permission_deep_link_url_for`
            // for the per-OS dispatch table.
            commands::calendar_permission_deep_link_url,
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
            // §X-3 — per-OS user-facing label of the OS credential
            // store. Returns the platform-specific name
            // (Keychain / secret-service / Credential Manager) for
            // the wizard's "store SSH key" tooltip. Pure function,
            // no I/O.
            commands::credential_store_name,
            // §X-5 / Phase 11 §11.1 — typed `KeyringHint` probe
            // for the wizard's SSH-key settings panel
            // (`SshKeySettings.svelte`). The frontend branches on
            // `hint.kind` to render one of 4 UI states (Empty /
            // PublicOnly / KeyPair / Unavailable) instead of
            // guessing from a missing `Some` vs. `None`. The
            // pure-function seam is
            // `keyring::keyring_hint_for(has_public, has_private)`
            // — see `commands::keyring_hint` + `keyring.rs` for
            // the per-OS probe implementation.
            commands::keyring_hint,
            // Phase 6 §6.6 — install-wizard's "do this later"
            // option. Appends `collector_id` to the
            // `pending_installs` array in `~/.trail/config.json`
            // (idempotent).
            install::mark_pending_install,
            // Phase 9 §9.1 — lazy-init the collector orchestrator +
            // scheduler after the wizard writes the config. Invoked
            // by the Svelte `<Onboarding />` `StepFinish` handler.
            // If called before the wizard finishes, returns a clean
            // Err ("config not found") instead of a runtime crash.
            start_collectors,
            // Phase 9 §9.2 — show the main window. Invoked by the
            // tray-icon "Show Trail" menu item (via the
            // `on_menu_event` closure in the setup above) and the
            // future wizard "Open settings" button. No-op if no
            // webview window labeled "main" exists yet (very early
            // boot, before §9.3 wires the main window).
            show_main_window,
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

    // ====================================================================
    // Phase 9 §9.1 tests — `start_collectors_inner` is the test-only
    // entry point that mirrors what the `start_collectors` IPC command
    // does after resolving the config path. Two tests cover both the
    // "wizard not yet finished" case (no config → Err) and the
    // "wizard just wrote a config" case (valid config → Ok + scheduler
    // task alive).
    // ====================================================================

    use super::start_collectors_inner;

    /// `start_collectors_inner` returns Err when the config does not
    /// exist yet. This is the safety check for the wizard: a buggy
    /// frontend firing `start_collectors` before the wizard finishes
    /// gets a clean error message instead of a silent orchestrator
    /// crash or a runtime panic.
    #[test]
    fn start_collectors_without_config_returns_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let config_path = tmp.path().join(".trail").join("config.json");
        assert!(
            !config_path.is_file(),
            "precondition: no config on disk at {}",
            config_path.display()
        );

        // The inner fn takes the path the IPC layer would compute —
        // we point it at a real-looking but missing file.
        let result = start_collectors_inner(&config_path);
        assert!(result.is_err(), "expected Err when config missing");
        let err_msg = result.err().unwrap();
        assert!(
            err_msg.contains("config") || err_msg.to_lowercase().contains("not found"),
            "error should mention missing config, got: {err_msg}"
        );
    }

    /// `start_collectors_inner` builds the orchestrator + spawns the
    /// scheduler when a valid config exists. Asserts `Ok` is returned
    /// and the spawned scheduler task is still alive after 2s (the
    /// same signal §9.6's e2e asserts: the scheduler logs "collector
    /// scheduler started" then parks until teardown).
    ///
    /// The tracing capture uses a `MakeWriter` shim so we can assert
    /// the canonical log line. The init is `try_init` because other
    /// tests in the suite may have already installed a subscriber.
    #[test]
    fn start_collectors_with_config_spawns_scheduler() {
        use std::sync::{mpsc, Arc, Mutex};
        use tracing_subscriber::fmt::MakeWriter;

        let tmp = tempfile::tempdir().expect("tempdir");
        let config_dir = tmp.path().join(".trail");
        std::fs::create_dir_all(&config_dir).expect("mkdir config dir");
        let config_path = config_dir.join("config.json");
        // Minimal valid config — fields + shape per `config::Config`
        // (every required field present, matching the unit-test
        // fixture in `src/config.rs::tests::load_valid_ssh_config`).
        let minimal_config = r#"{
            "claude_sessions_paths": [],
            "github": {"mode": "gh_cli", "host": "github.com"},
            "calendar_ics": "/nonexistent.ics",
            "calendar": {"kind": "ics", "path": "/nonexistent.ics"},
            "voice": {"enabled": true, "hotkey": "ctrl+shift+space", "transcriber": "whisper_cpp", "model": "base.en"},
            "review_time": "18:00",
            "summarizer": {"model": "gpt-oss:20b", "model_provider": "local", "anonymization_strictness": "aggressive", "use_generic_categories": true},
            "transport": {"type": "ssh", "host": "vm.example.com", "port": 22, "user": "trail", "auth": {"auth": "public_key", "path": "/tmp/trail-test-key"}, "remote_path": "/tmp/trail-remote"},
            "raw_retention_days": 7,
            "pending_installs": []
        }"#;
        std::fs::write(&config_path, minimal_config).expect("write minimal config");
        assert!(config_path.is_file(), "precondition: config on disk");

        // Capture tracing output to assert the "collector scheduler
        // started" log line. Use a `MakeWriter` shim so tracing's
        // worker thread can drain into our channel.
        let (tx, rx) = mpsc::channel::<String>();
        struct TxWriter(Arc<Mutex<mpsc::Sender<String>>>);
        impl<'a> MakeWriter<'a> for TxWriter {
            type Writer = TxGuard;
            fn make_writer(&'a self) -> TxGuard {
                TxGuard(self.0.clone())
            }
        }
        struct TxGuard(Arc<Mutex<mpsc::Sender<String>>>);
        impl std::io::Write for TxGuard {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                let _ = self
                    .0
                    .lock()
                    .unwrap()
                    .send(String::from_utf8_lossy(buf).to_string());
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let tx_writer = TxWriter(Arc::new(Mutex::new(tx)));
        let _ = tracing_subscriber::fmt()
            .with_writer(tx_writer)
            .with_max_level(tracing::Level::INFO)
            .try_init();

        // The inner fn spawns a scheduler task via `tokio::spawn`;
        // that requires a tokio runtime. Build a multi-threaded
        // runtime and KEEP IT ALIVE in scope until we've drained the
        // tracing channel — dropping the runtime aborts the
        // parking task inside the spawned scheduler.
        let runtime = std::mem::ManuallyDrop::new(
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime"),
        );
        // SAFETY: we never drop `runtime` until end of scope; the
        // pinned `JoinHandle` in `_sched_task_box` keeps the worker
        // thread from being reaped before we observe the log line.
        let (orch, sched_task) = {
            let rt_handle = runtime.handle();
            let _guard = rt_handle.enter();
            start_collectors_inner(&config_path).expect("start_collectors_inner ok")
        };
        // Keep the orch alive until the test ends so its scheduler
        // task doesn't get reaped; the parking future inside the
        // spawned task lets the runtime workers spin down on the
        // next idle. The scheduler logs "collector scheduler
        // started" BEFORE parking — that's the signal we assert on.
        let _orch_keep_alive = orch;

        // Wait up to 2s for the canonical log line. The scheduler
        // first runs `start().await`, then logs the line and parks;
        // we drain the channel until we see it. We sleep on this
        // thread so the runtime workers can drive the spawned task
        // forward.
        let started = {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            let mut found = false;
            while std::time::Instant::now() < deadline {
                if let Ok(line) = rx.recv_timeout(std::time::Duration::from_millis(100)) {
                    if line.contains("collector scheduler started") {
                        found = true;
                        break;
                    }
                }
            }
            found
        };
        assert!(
            started,
            "expected 'collector scheduler started' in tracing output within 2s"
        );

        // The scheduler task should still be alive (it's parked in
        // `pending::<()>().await` until Tauri drops the runtime).
        assert!(
            !sched_task.is_finished(),
            "scheduler task should still be parked, not finished"
        );
        // Drop the JoinHandle first so the runtime can drain it
        // before we let `runtime` go out of scope and shut down.
        drop(sched_task);
    }
}
