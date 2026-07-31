// `mod config;` lands in Phase 1 §1.2 (laptop config loader).
// `mod keyring;` lands in Phase 1 §1.3 (macOS Keychain keypair generator).
// `mod transport;` lands in Phase 1 §1.4 (SSH transport + IPC bindings).
// They are added incrementally to the workspace below.

mod config;
mod keyring;

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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            greet,
            get_config,
            generate_ssh_key,
            get_ssh_public_key
        ])
        .run(tauri::generate_context!())
        .expect("error while running trail");
}
