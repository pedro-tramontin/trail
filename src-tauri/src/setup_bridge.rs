//! Phase 9 §9.1 — bridge between the Tauri runtime and
//! `crate::start_collectors_inner`. The command wrapper lives here
//! rather than at the crate root so that `tauri-macros` 2.6.3's
//! `#[macro_export]` on `pub`-visible command functions doesn't
//! collide at the crate root (where the macro_rules! definition AND
//! the visibility-scoped `use` re-export would otherwise land at
//! the same path, triggering an `E0255` "defined multiple times"
//! on `__cmd__start_collectors`).
//!
//! Putting the command in its own module scopes both the macro
//! definition and the macro_export re-export to that module.
//! `crate::start_collectors` at the lib.rs root is a thin proxy
//! that calls into here; the `invoke_handler!` macro list picks
//! the lib-root proxy so the IPC contract name stays
//! "start_collectors".

use std::sync::Arc;
use tauri::Manager;

/// Tauri command wrapper. Resolves the config path via the
/// platform-correct `AppHandle`, calls `start_collectors_inner`
/// to bring up the orchestrator + scheduler, then flips the
/// `ConfigState` to `Ready(cfg)` so subsequent IPC commands see a
/// live orchestrator.
#[tauri::command]
pub fn start_collectors(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app_config_dir: {e}"))?;
    let config_path = dir.join("config.json");
    let (orch, sched_task) = crate::start_collectors_inner(&config_path)?;
    // Re-load to capture the parsed config + flip the state machine
    // from `AwaitingOnboarding` to `Ready(...)` so subsequent IPC
    // commands see a live orchestrator.
    let cfg = crate::config::load_config(&config_path)
        .map_err(|e| format!("loading config from {}: {e}", config_path.display()))?;
    app.manage(orch);
    app.manage(Arc::new(sched_task));
    app.manage(crate::ConfigState::Ready(cfg));
    Ok(())
}
