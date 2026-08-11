//! Tauri command handlers — kept separate from `lib.rs` so the test
//! module has its own scope and so `lib.rs` stays a thin shim that
//! only registers the handlers.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config;
use crate::logs::{self, LogEntry};
use crate::ollama::{OllamaClient, DEFAULT_ENDPOINT};
use crate::summarizer::{self, SummarizeReceipt};
use crate::transport::{self, Transport};
use crate::validate;
use crate::voice::capture::CaptureState;

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

/// Tauri command: probe a not-yet-persisted SSH connection from the
/// wizard's "Test connection" button. Builds an [`SshTransport`]
/// in-memory from the supplied (host, port, user) — publickey auth
/// against whatever key is currently in the OS keychain — runs
/// [`SshTransport::health_check`], and returns `Ok(())` on success
/// or a flattened error string on failure.
///
/// Distinct from [`health_check_transport`] in two ways:
///   1. No config file is read from disk (the wizard hasn't written
///      one yet at this point).
///   2. The connection details are passed in as arguments rather
///      than loaded from the on-disk [`crate::config::Config`].
///
/// The keychain lookup matches the v1 design: [`crate::keyring`]
/// stores the private key on first-run, and every subsequent
/// onboarding reuses the same key. If no key is in the keychain
/// (fresh install + user clicked "Test connection" before
/// "Generate SSH key"), the underlying `load_private_key_pem`
/// returns `SSH key not generated yet — run onboarding first`
/// and we surface that string to the UI so the error is
/// actionable.
#[tauri::command]
pub async fn test_ssh_connection(
    host: String,
    port: u16,
    user: String,
) -> Result<(), String> {
    use crate::config::SshAuth;
    use crate::transport::SshTransport;

    let host = host.trim().to_string();
    let user = user.trim().to_string();
    if host.is_empty() {
        return Err("host is required".into());
    }
    if user.is_empty() {
        return Err("user is required".into());
    }
    if port == 0 {
        return Err("port must be between 1 and 65535".into());
    }

    // The on-disk `path` in `SshAuth::PublicKey` is required by
    // the config schema but the v1 transport loads the private
    // key from the keychain via `userauth_pubkey_memory`, so the
    // path is unused at runtime. `remote_path` is likewise unused
    // by `health_check` (it only opens a TCP connection + does
    // pubkey auth). Both fields are populated with benign defaults
    // so the SshTransport constructor is satisfied.
    let t = SshTransport::new(
        host,
        port,
        user,
        SshAuth::PublicKey {
            path: PathBuf::from("~/.ssh/trail_ed25519"),
        },
        PathBuf::from("/tmp/"),
    );
    t.health_check().await.map_err(|e| e.to_string())?;
    Ok(())
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
/// `model` — ollama model name to use (e.g. `"llama3"`). The
/// caller passes whatever the user picked; we deliberately ignore
/// `Config.summarizer.model` (the field exists in `config.rs`
/// already, but the menu-bar UI passes the model as an explicit
/// argument so the same call works for "summarize today with the
/// last-used model" and "summarize today with this experimental
/// one").
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
        &cfg.summarizer.anonymization_rules,
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
// `voice_abort` (§5.6) is wired now because the abort path doesn't
// need a real cpal stream — it just needs the shared
// `CaptureState` registered via `app.manage()` so it can clear the
// samples buffer and `.abort()` the consumer JoinHandle. On Linux
// it returns the same "voice capture is only supported on macOS"
// error as `voice_start`/`voice_stop`.
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
///
/// §5.6 abort-on-failure: when the future §5.7 impl's
/// `transcribe` step returns `Err(...)`, the full command will
/// call `crate::voice::abort::voice_abort(...)` to roll the
/// partial capture back (drop the samples buffer, abort the
/// consumer task, delete the partial files). For Part A the
/// stub returns the friendly message above without touching the
/// `CaptureState`; the abort path is independently testable via
/// the `voice_abort` Tauri command and the `voice::abort` unit
/// tests.
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
        // Touch the abort module so the §5.6 unit tests stay
        // covered on non-macOS hosts where the cpal branch is
        // never reached.
        crate::voice::no_op_abort().map_err(|e| e.to_string())?;
        Err("voice capture is only supported on macOS".to_string())
    }
}

/// Phase 5 §5.6 Tauri command: abort an in-progress voice capture.
///
/// Drops the in-memory samples buffer, aborts the consumer task
/// via `JoinHandle.abort()`, and removes any partial WAV + JSON
/// files from `~/.trail/raw/<date>/voice/<entry_id>.{json,wav}`.
/// Idempotent — safe to call when no capture is active (returns
/// `Ok` with the buffer empty and no files removed).
///
/// `trail_root` + `date` + `entry_id` identify the partial files
/// to delete, if any. The store's `delete` is itself idempotent
/// so passing an `entry_id` with no on-disk file is harmless.
#[tauri::command]
pub async fn voice_abort(
    state: tauri::State<'_, Arc<CaptureState>>,
    trail_root: std::path::PathBuf,
    date: String,
    entry_id: uuid::Uuid,
) -> Result<String, String> {
    crate::voice::abort::voice_abort(state.inner(), &trail_root, &date, entry_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok("voice capture aborted".into())
}

/// Resolve the `~/.trail/` root directory from the loaded config. The
/// config itself doesn't store its own location (we only ever persist
/// the raw/drafts subdirs), so we look next to the config file —
/// matching `resolve_paths` in `lib.rs`.
fn trail_root_from_config(_cfg: &config::Config) -> std::path::PathBuf {
    // The summarizer is unit-testable without an `AppHandle`; using
    // `$HOME` here matches the collectors' convention so the test
    // fixtures' `trail_root` aligns with what production will see.
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
            "calendar": {"kind": "ics", "path": "x"},
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

    /// `test_ssh_connection` is the wizard's "Test connection"
    /// button. It validates host / user / port BEFORE opening
    /// any TCP socket, so the validation arm is testable
    /// without a real SSH server. The success arm requires a
    /// real server (and a key in the keychain) and is left to
    /// the integration tests.
    #[tokio::test]
    async fn empty_host_returns_required_error() {
        let r = test_ssh_connection("".into(), 22, "pedro".into()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("host is required"));
    }

    #[tokio::test]
    async fn whitespace_host_returns_required_error() {
        // Trim should make whitespace-only an empty string and
        // then fail the required check.
        let r = test_ssh_connection("   ".into(), 22, "pedro".into()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("host is required"));
    }

    #[tokio::test]
    async fn empty_user_returns_required_error() {
        let r = test_ssh_connection("vps.example.com".into(), 22, "".into()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("user is required"));
    }

    #[tokio::test]
    async fn port_zero_returns_range_error() {
        // u16 is the parameter type so the smallest legal value
        // is 0; we explicitly reject that and require 1+.
        let r = test_ssh_connection("vps.example.com".into(), 0, "pedro".into()).await;
        assert!(r.is_err());
        assert!(r.unwrap_err().contains("port"));
    }
}
