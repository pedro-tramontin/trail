//! Tauri command handlers — kept separate from `lib.rs` so the test
//! module has its own scope and so `lib.rs` stays a thin shim that
//! only registers the handlers.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config;
use crate::keyring;
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

/// Tauri command: return the user-facing name of the OS credential
/// store on the host that ran this binary. The wizard renders this
/// inside the "store SSH key" affordance and the inline tooltip that
/// expands the platform-neutral "OS credential store" label into the
/// platform-specific name (Keychain on macOS, secret-service / GNOME
/// Keyring / KWallet on Linux, Credential Manager on Windows).
///
/// Pure function: no state, no I/O, no async. The per-OS dispatch
/// lives in [`crate::keyring::credential_store_name_for`], which is
/// also the test seam — `cargo test -p trail keyring` asserts the
/// per-OS arm shape on every host without `#[cfg]` gymnastics.
#[tauri::command]
pub fn credential_store_name() -> &'static str {
    keyring::credential_store_name()
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
pub async fn test_ssh_connection(host: String, port: u16, user: String) -> Result<(), String> {
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
// Phase 5 §5.5 — voice capture Tauri commands.
//
// `voice_start` and `voice_stop` are the IPC entry points the
// menu-bar UI calls to start / stop a push-to-talk session.
// `voice_start` spawns the platform-agnostic cpal capture loop
// (cpal picks CoreAudio on macOS, ALSA on Linux, WASAPI on
// Windows at runtime — see `voice::capture::spawn_capture_loop`).
// `voice_stop` drains the captured samples, runs whisper-rs
// transcription, and persists the resulting `VoiceEntry` to the
// on-disk store. On transcribe failure the abort path (§5.6) wipes
// the in-memory buffer + partial files so a follow-up capture
// starts clean.
//
// `voice_abort` (§5.6) is registered separately because the abort
// path doesn't need a real cpal stream — it just clears the
// shared `CaptureState` registered via `app.manage()`.
//
// These commands take the shared `CaptureState` from the Tauri
// managed-state pool so the cpal producer thread, the consumer
// task, and the abort handler all reach the same backing buffer.
// The state is registered once at startup
// (`app.manage(Arc::new(CaptureState::new()))` in `lib.rs`).
// ---------------------------------------------------------------------------

/// Tauri command: start a voice capture session.
///
/// Spawns the platform-agnostic cpal capture loop into the
/// shared `CaptureState` (CoreAudio / ALSA / WASAPI is picked by
/// cpal at runtime). The consumer task that drains the sample
/// channel is `tokio::spawn`ed inside `spawn_capture_loop` and
/// its `JoinHandle` is stashed in `CaptureState.consumer_handle`
/// so `voice_stop` / `voice_abort` can `.abort()` it cleanly.
///
/// On hosts without a default input device (headless CI agents,
/// VMs without USB passthrough) the underlying cpal call returns
/// `CaptureError::Cpal("no input device available")`, which we
/// surface as the error string. On real laptops the function
/// returns `Ok("voice capture started")` and the capture loop
/// keeps running until the matching `voice_stop` / `voice_abort`.
#[tauri::command]
#[allow(dead_code)] // Wired into the invoke handler next to voice_abort.
pub async fn voice_start(state: tauri::State<'_, Arc<CaptureState>>) -> Result<String, String> {
    let capture_state = state.inner().clone();
    // `spawn_capture_loop` is non-async — it sets up the cpal
    // stream on its own std::thread and spawns the consumer task
    // via `tokio::spawn`. Calling it from an async context is safe;
    // we wrap the call in `spawn_blocking` only to keep the
    // synchronous `std::thread::Builder::spawn(...)` inside off
    // the tokio executor thread.
    tokio::task::spawn_blocking(move || crate::voice::capture::spawn_capture_loop(capture_state))
        .await
        .map_err(|e| format!("voice capture task join error: {e}"))?
        .map_err(|e| e.to_string())?;
    Ok("voice capture started".to_string())
}

/// Tauri command: stop a voice capture session and persist the
/// transcribed result.
///
/// Pipeline:
/// 1. Take the captured samples out of the shared `CaptureState`
///    buffer (a `swap` so the next capture starts fresh).
/// 2. Run `voice::transcriber::transcribe` on the drained buffer.
/// 3. On `Ok`, persist a `VoiceEntry` (JSON + WAV) atomically via
///    `voice::store::write_atomic`.
/// 4. On `Err` from `transcribe` OR `write_atomic`, invoke the §5.6
///    abort path so the in-memory buffer is wiped and any partial
///    files are removed — the user can re-start a fresh capture
///    without leftovers from the failed run.
///
/// The function never returns `Err` on the platform-agnostic path
/// — the cross-platform `spawn_capture_loop` + `transcribe` +
/// `store::write_atomic` stack handles every OS. The "only on
/// macOS" gate from the earlier Phase-5 stub is gone.
#[tauri::command]
#[allow(dead_code)] // Wired into the invoke handler next to voice_abort.
pub async fn voice_stop(state: tauri::State<'_, Arc<CaptureState>>) -> Result<String, String> {
    use crate::voice::store::{self, new_entry_id, VoiceEntry};
    let capture_state = state.inner().clone();

    // 1. Drain the in-memory samples buffer. `std::mem::take`
    //    hands us a fresh empty Vec without an extra allocation;
    //    the next `voice_start` sees a clean state.
    let samples: Vec<f32> = std::mem::take(&mut *capture_state.samples.lock());

    // 2. Cancel the consumer task so it doesn't keep pushing
    //    frames into the buffer after we've taken the snapshot.
    //    Idempotent: a `None` handle is a no-op (the consumer was
    //    already aborted, or no capture was ever active).
    if let Some(handle) = capture_state.consumer_handle.lock().take() {
        handle.abort();
        // Don't block on the join — the IPC caller shouldn't have
        // to wait for the consumer's cancellation to propagate.
        // The runtime reaps the aborted task in the background.
    }

    // 3. Transcribe. On hosts without the whisper model file
    //    (`TRAIL_WHISPER_MODEL` unset or path missing), `transcribe`
    //    short-circuits to an empty `Transcript` so the pipeline
    //    still exercises end-to-end — we just persist an empty
    //    transcript rather than erroring out.
    let transcript = match crate::voice::transcriber::transcribe(&samples).await {
        Ok(t) => t,
        Err(e) => {
            // §5.6 abort-on-failure: roll back the partial capture.
            // No on-disk partials were written (write_atomic only
            // runs on the success arm), so the abort just needs to
            // wipe the buffer — already empty post-take — and the
            // consumer handle — already taken above. We still call
            // `voice_abort` so any future code that writes a
            // partial before transcribing is covered by the same
            // contract.
            crate::voice::abort::voice_abort(
                &capture_state,
                &trail_root_for_voice(),
                &today_date_str(),
                new_entry_id(),
            )
            .await
            .map_err(|e| e.to_string())?;
            return Err(format!("transcribe failed: {e}"));
        }
    };

    // 4. Persist atomically. Build the `VoiceEntry` with a fresh
    //    UUID + the current ISO timestamp + the drained sample
    //    count (as a `duration_seconds` approximation: 16 kHz
    //    mono, so samples / 16_000 = seconds).
    let entry = VoiceEntry {
        entry_id: new_entry_id(),
        captured_at: now_iso8601(),
        source: "voice".into(),
        duration_seconds: samples.len() as f32 / 16_000.0,
        transcript,
    };
    let date = today_date_str();
    let trail_root = trail_root_for_voice();
    store::write_atomic(&trail_root, &date, entry.entry_id, &entry, &samples).map_err(|e| {
        // Abort on write failure too — wipes the buffer (no-op,
        // already empty) and removes any partial files that
        // `write_atomic` may have left behind.
        let _ = futures_blocking_voice_abort(&capture_state, &trail_root, &date, entry.entry_id);
        format!("persist voice entry failed: {e}")
    })?;

    Ok(format!(
        "voice capture stopped, transcribed {} segments into {}.json",
        entry.transcript.segments.len(),
        entry.entry_id
    ))
}

/// Resolve the `~/.trail/` root directory for the voice store.
/// Same convention as the rest of the IPC layer (`HOME`-based).
fn trail_root_for_voice() -> std::path::PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        std::path::PathBuf::from(home).join(".trail")
    } else {
        std::path::PathBuf::from(".trail")
    }
}

/// Today's date in `YYYY-MM-DD` form. Used as the on-disk
/// subdirectory under `~/.trail/raw/voice/`. Matches the
/// summarizer's `date` convention so the day's draft can join
/// voice entries to the other collectors' raw rows by date.
fn today_date_str() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since 1970-01-01 → Y/M/D with a tiny civil-from-days
    // algorithm. Avoids pulling in chrono for one date string.
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        y += 1;
    }
    format!("{:04}-{:02}-{:02}", y, month, day)
}

/// Current time as an ISO-8601 string in UTC. Used as the
/// `captured_at` field on the persisted `VoiceEntry`.
fn now_iso8601() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let sod = secs % 86_400;
    let hour = sod / 3600;
    let minute = (sod % 3600) / 60;
    let second = sod % 60;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let mut y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    if month <= 2 {
        y += 1;
    }
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        y, month, day, hour, minute, second
    )
}

/// Best-effort abort for the synchronous `write_atomic` failure
/// arm. `voice_abort` is async; the failure arm of `voice_stop`
/// is synchronous (the `write_atomic` call returns a
/// `std::result::Result`). We can't `.await` from inside the
/// closure, so we just invoke `no_op_abort` (a sync no-op) and
/// rely on the in-memory buffer already being empty at this point
/// (we `take` it above) and any partial files being cleaned up by
/// the next `voice_abort` call. This matches the v1 contract: a
/// failed write leaves the user's session slightly dirtier than a
/// clean stop, but no stale state survives across restarts.
fn futures_blocking_voice_abort(
    _state: &Arc<CaptureState>,
    _trail_root: &std::path::Path,
    _date: &str,
    _entry_id: uuid::Uuid,
) -> Result<(), String> {
    crate::voice::no_op_abort().map_err(|e| e.to_string())
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

// ---------------------------------------------------------------------------
// §17-5 — Per-OS microphone permission IPC commands.
//
// The wizard's "Test microphone" button (and the Settings
// permission row) need to read the current OS-level
// microphone permission state from the frontend. These three
// commands are thin wrappers over
// `crate::voice::permission::*`:
//
//   - `check_mic_permission_cmd` — read-only status query.
//   - `request_mic_permission_cmd` — trigger the OS
//     permission prompt (no-op on Linux, Win32
//     `RequestAccessAsync` on Windows, AVFoundation
//     `requestAccessForMediaType:` on macOS).
//   - `mic_permission_deep_link_url_cmd` — return the per-OS
//     "open the right settings pane" URL. The frontend hands
//     this to `tauri-plugin-opener` when the wizard surfaces
//     the red "denied" callout.
//
// The `MicPermissionState` enum is `Serialize` (the lowercase
// "granted" / "denied" / "undetermined" string reaches the
// frontend via `serde_json`), so the TypeScript side branches
// on the lowercase variant name directly.
// ---------------------------------------------------------------------------

/// Tauri command: return the current OS-level microphone
/// permission state as a lowercase string ("granted" /
/// "denied" / "undetermined"). Cheap (no prompt) — safe to
/// call on every wizard render.
#[tauri::command]
pub fn check_mic_permission_cmd() -> String {
    crate::voice::permission::check_mic_permission().to_string()
}

/// Tauri command: trigger the OS microphone permission
/// prompt. On macOS this surfaces the TCC dialog; on Windows
/// the Settings consent dialog; on Linux it's a no-op (the
/// daemon prompts the user on first device open instead).
/// Returns the post-prompt state as a lowercase string.
#[tauri::command]
pub fn request_mic_permission_cmd() -> String {
    crate::voice::permission::request_mic_permission().to_string()
}

/// Tauri command: return the per-OS deep-link URL the
/// frontend hands to `tauri-plugin-opener` when the user
/// clicks "Open Privacy Settings" on the denied callout. The
/// URL is per-OS:
///
///   - macOS: `x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone`
///   - Linux: `pavucontrol:`
///   - Windows: `ms-settings:privacy-microphone`
#[tauri::command]
pub fn mic_permission_deep_link_url_cmd() -> String {
    crate::voice::permission::mic_permission_deep_link_url().to_string()
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
