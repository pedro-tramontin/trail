//! Tauri command handlers — kept separate from `lib.rs` so the test
//! module has its own scope and so `lib.rs` stays a thin shim that
//! only registers the handlers.

use std::path::PathBuf;

use crate::config;
use crate::transport::{self, Transport};
use crate::validate;

/// Build a `Box<dyn Transport>` from the laptop config on disk. Both
/// error sources are flattened to `String` so the frontend sees a
/// single `Result<_, String>` shape (Tauri requires that form for
/// command return types).
pub fn build_transport(config_path: PathBuf) -> Result<Box<dyn Transport>, String> {
    let cfg = config::load_config(&config_path).map_err(|e| e.to_string())?;
    transport::from_config(&cfg.transport).map_err(|e| e.to_string())
}

/// Tauri command: probe the configured transport (SSH reachability +
/// publickey auth). No file push; just proves the transport can be
/// exercised end-to-end. Returns the transport's friendly name on
/// success so the wizard can display "connected via ssh".
#[tauri::command]
pub async fn health_check_transport(config_path: String) -> Result<String, String> {
    let t = build_transport(PathBuf::from(config_path))?;
    t.health_check().await.map_err(|e| e.to_string())?;
    Ok(t.name().to_string())
}

/// Tauri command: push a serialized day summary to the VPS via the
/// configured transport. `payload` is the raw bytes (JSON in
/// practice); `remote_name` is the filename the VPS side will see
/// (typically `<date>.json`).
#[tauri::command]
pub async fn push_to_vps(
    config_path: String,
    payload: Vec<u8>,
    remote_name: String,
) -> Result<(), String> {
    let t = build_transport(PathBuf::from(config_path))?;
    t.push(&payload, &remote_name)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Tauri command: validate a day-summary payload against the bundled
/// `day-summary.schema.json` schema BEFORE pushing to the VPS. Catches
/// schema violations client-side so the user sees a clean error list
/// in the wizard, not a confused "collector rejected the file" later.
///
/// On success returns `Ok(())`. On failure returns
/// `Err("<newline-separated list of validation errors>")`. The list is
/// sorted + deduped by the underlying validator (see
/// `validate::validate`); we flatten to a `String` for the existing
/// `Result<_, String>` Tauri command shape. Newlines separate the
/// errors so the frontend can split + display them as a list.
///
/// The schema is loaded at compile time via `include_str!` — see the
/// doc comment on `validate::compiled_schema` for the trade-off vs.
/// `app.path().resource_dir()`.
#[tauri::command]
pub fn validate_day_summary(payload: serde_json::Value) -> Result<(), String> {
    validate::validate(&payload).map_err(|e| e.errors.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_config(json: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn missing_config_returns_friendly_error() {
        // The error path goes through `config::load_config`'s `NotFound`
        // branch; that wraps to a friendly message string the frontend
        // can surface to the user (no stack trace, no filesystem path
        // leakage beyond the path itself).
        let result = build_transport(PathBuf::from("/nonexistent/path/config.json"));
        assert!(result.is_err(), "expected an error for missing config");
        let err = result.unwrap_err();
        assert!(
            err.contains("config file not found"),
            "expected friendly 'config file not found' in error, got: {err:?}"
        );
    }

    #[test]
    fn invalid_transport_type_returns_config_error() {
        // JSON is valid + parseable into `Config`, but `transport`'s
        // `type` tag is `"https"` — not a `TransportConfig` variant.
        // Serde returns an "unknown variant" error, which surfaces as
        // a non-empty error string. We don't pin the exact message
        // (serde's wording drifts across versions) but we do assert
        // the error gets routed through `config::load_config`'s
        // `InvalidJson` arm and not e.g. swallowed to Ok.
        let json = r#"{
            "claude_sessions_paths": [],
            "github": {"mode": "gh_cli", "host": "x"},
            "calendar_ics": "x",
            "voice": {"enabled": false, "hotkey": "x", "transcriber": "x", "model": "x"},
            "review_time": "18:00",
            "summarizer": {"model": "x", "model_provider": "x", "anonymization_strictness": "x", "use_generic_categories": false},
            "transport": {"type": "https", "url": "https://x"},
            "raw_retention_days": 1,
            "pending_installs": []
        }"#;
        let f = write_temp_config(json);
        let result = build_transport(f.path().to_path_buf());
        assert!(result.is_err(), "expected error for unknown transport type");
        // The error string is non-empty and comes from serde's unknown-variant message.
        let err = result.unwrap_err();
        assert!(!err.is_empty(), "error string must not be empty");
    }
}
