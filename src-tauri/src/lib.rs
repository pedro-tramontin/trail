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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            greet,
            get_config,
            generate_ssh_key,
            get_ssh_public_key,
            commands::health_check_transport,
            commands::push_to_vps,
            commands::validate_day_summary,
            list_collectors,
            run_collector_now,
            set_collector_enabled,
        ])
        .run(tauri::generate_context!())
        .expect("error while running trail");
}
