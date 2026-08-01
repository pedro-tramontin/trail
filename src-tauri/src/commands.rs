//! Tauri command handlers — kept separate from `lib.rs` so the test
//! module has its own scope and so `lib.rs` stays a thin shim that
//! only registers the handlers.

use std::path::PathBuf;

use crate::config;
use crate::logs::{self, LogEntry};
use crate::ollama::{OllamaClient, DEFAULT_ENDPOINT};
use crate::summarizer::{self, SummarizeReceipt};
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

/// Tauri command: build the per-day draft from the raw collector
/// JSON. Reads the config (for `trail_root` + model resolution),
/// spins up a fresh `OllamaClient` against the default endpoint,
/// delegates to `summarizer::run`, and surfaces the receipt (or a
/// flattened error string) to the frontend.
///
/// `model` — ollama model name to use (e.g. `"llama3"`). For v1 the
/// caller passes whatever they configured; the command doesn't read
/// `Config.summarizer.model` because that field lands in Phase 1 §1.x
/// and the menu-bar UI calls this with a user-picked value.
#[tauri::command]
pub async fn summarize_day(
    config_path: String,
    date: String,
    model: String,
) -> Result<SummarizeReceipt, String> {
    let cfg = config::load_config(std::path::Path::new(&config_path)).map_err(|e| e.to_string())?;
    let trail_root = trail_root_from_config(&cfg);
    let raw_root = trail_root.join("raw");
    let drafts_dir = trail_root.join("drafts");
    let strictness = cfg.summarizer.anonymization_strictness.as_str();
    let client = OllamaClient::new(DEFAULT_ENDPOINT);
    let bootstrap_path = trail_root.join("summary_bootstrap.json");
    summarizer::run(
        &raw_root,
        &drafts_dir,
        &bootstrap_path,
        &date,
        &model,
        strictness,
        &client,
    )
    .await
    .map_err(|e| e.to_string())
}

/// Tauri command: record a single user-driven edit to a draft section
/// (called by the Review UI whenever Pedro edits a section or
/// unchecks an item). Classifies the (before, after) pair into a
/// [`crate::learner::LearningKind`], appends it to the
/// `summary_bootstrap.json` file, and returns the new total rule
/// count. The `section` argument is currently unused by the
/// classifier but is reserved for a future version that scopes rules
/// per `## ` heading.
#[tauri::command]
pub async fn record_review_diff(
    config_path: String,
    section: String,
    before: String,
    after: String,
) -> Result<usize, String> {
    use crate::learner::{classify, record_event};
    let cfg = config::load_config(std::path::Path::new(&config_path)).map_err(|e| e.to_string())?;
    let trail_root = trail_root_from_config(&cfg);
    let bootstrap_path = trail_root.join("summary_bootstrap.json");
    let kind = classify(&before, &after);
    // v1: treat the whole before/after as a single "pattern → replacement"
    // rule. A future v2 might tokenize into smaller fragments; the
    // `section` argument is held back for that work.
    let _ = section; // currently unused; see doc comment above.
    let bootstrap =
        record_event(&bootstrap_path, kind, &before, &after).map_err(|e| e.to_string())?;
    Ok(bootstrap.rules.len())
}

/// Phase 4 §4.1 Tauri command: list every raw collector file for
/// a given day, sorted by `captured_at` ascending. The frontend
/// uses this to render the Logs UI rows. Returns an empty Vec
/// when the day's directory doesn't exist — missing days are not
/// an error (the UI should show an empty state).
#[tauri::command]
pub async fn list_logs(config_path: String, date: String) -> Result<Vec<LogEntry>, String> {
    let cfg = config::load_config(std::path::Path::new(&config_path)).map_err(|e| e.to_string())?;
    let trail_root = trail_root_from_config(&cfg);
    logs::list_logs(&trail_root, &date).map_err(|e| e.to_string())
}

/// Phase 4 §4.1 Tauri command: delete the raw collector file for
/// `(date, source)`. Idempotent — calling it on a missing file is
/// a no-op (so the UI's "delete" button stays safe on re-click).
#[tauri::command]
pub async fn delete_log(config_path: String, date: String, source: String) -> Result<(), String> {
    let cfg = config::load_config(std::path::Path::new(&config_path)).map_err(|e| e.to_string())?;
    let trail_root = trail_root_from_config(&cfg);
    logs::delete_log(&trail_root, &date, &source).map_err(|e| e.to_string())
}

/// Phase 4 §4.1 Tauri command: read + parse the raw JSON file for
/// `(date, source)`. Returns the parsed `serde_json::Value` so the
/// frontend can pretty-print, schema-validate, or diff against the
/// draft.
#[tauri::command]
pub async fn get_raw_json(
    config_path: String,
    date: String,
    source: String,
) -> Result<serde_json::Value, String> {
    let cfg = config::load_config(std::path::Path::new(&config_path)).map_err(|e| e.to_string())?;
    let trail_root = trail_root_from_config(&cfg);
    logs::get_raw_json(&trail_root, &date, &source).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Phase 5 §5.5 (Part A) — voice capture Tauri commands.
//
// These are stubs: `voice_start` and `voice_stop` are the IPC entry
// points the menu-bar UI will call to start/stop a push-to-talk
// session. The full impl (wiring the cpal capture loop into a
// shared `AppHandle` state, draining the sample channel into a ring
// buffer, kicking off the whisper run on stop, persisting the
// resulting `VoiceEntry` to disk) lands in §5.7 (Part B) once
// macOS TCC microphone permission is sorted out.
//
// For Part A we keep the surface stable: on macOS the command
// names exist (and return a friendly message) so the frontend
// binding compiles, but the heavy work is deferred. On Linux the
// commands return `Err("voice capture is only supported on macOS")`
// so the test suite can exercise them without a real microphone.
// ---------------------------------------------------------------------------

/// Phase 5 §5.5 Tauri command: start a voice capture session.
///
/// On macOS this would spawn the cpal capture loop, wire the audio
/// meter + tray-icon blink loop, and stash the receive end in
/// `AppState` so `voice_stop` can drain it. The full impl lives in
/// §5.7. For Part A we just acknowledge the request.
///
/// On non-macOS the command is rejected because cpal + global-hotkey
/// only build on macOS (see `[target.'cfg(target_os = "macos")'.dependencies]`
/// in `src-tauri/Cargo.toml`).
#[tauri::command]
#[allow(dead_code)] // Wired into the invoke handler in §5.7 (Part B).
pub async fn voice_start() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        // Touch the imports so the macOS-only deps are referenced
        // in the IPC layer (suppresses "unused import" warnings
        // until §5.7 lands).
        let _ = std::any::type_name::<crate::voice::capture::CaptureError>();
        Ok("voice capture starting (full impl in §5.7)".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("voice capture is only supported on macOS".to_string())
    }
}

/// Phase 5 §5.5 Tauri command: stop a voice capture session and
/// persist the result.
///
/// On macOS this would drain the sample channel, run
/// `voice::transcriber::transcribe`, and call
/// `voice::store::write_atomic` to persist the JSON + WAV pair.
/// The full impl lives in §5.7. For Part A we just acknowledge
/// the request.
#[tauri::command]
#[allow(dead_code)] // Wired into the invoke handler in §5.7 (Part B).
pub async fn voice_stop() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        let _ = std::any::type_name::<crate::voice::transcriber::TranscribeError>();
        Ok("voice capture stopping (full impl in §5.7)".to_string())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("voice capture is only supported on macOS".to_string())
    }
}

/// Resolve the `~/.trail/` root directory from the loaded config. The
/// config itself doesn't store its own location (we only ever persist
/// the raw/drafts subdirs), so we look next to the config file —
/// matching `resolve_paths` in `lib.rs`.
fn trail_root_from_config(_cfg: &config::Config) -> std::path::PathBuf {
    // Phase 3 convention: the trail root is the parent dir of
    // `config.json`, falling back to `~/.trail/` when the dir layout
    // is unknown. This avoids baking a Tauri-specific path resolution
    // into the summarizer (which is also unit-testable without an
    // `AppHandle`).
    if let Some(home) = std::env::var_os("HOME") {
        std::path::PathBuf::from(home).join(".trail")
    } else {
        std::path::PathBuf::from(".trail")
    }
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
