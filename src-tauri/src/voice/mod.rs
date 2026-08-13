//! Voice capture pipeline (Phase 5).
//!
//! `model_manager` (5-1) downloads + verifies the whisper GGML file the
//! first time any voice feature needs it. `capture` (5-2) opens the
//! microphone via cpal (CoreAudio / ALSA / WASAPI at runtime) and
//! resamples to 16 kHz mono via rubato. `hotkey` (5-3) parses the
//! push-to-talk shortcut string (`Ctrl+Shift+Space` by default) and
//! registers it via `global-hotkey` (Carbon / X11 / Win32 at runtime)
//! with conflict detection. `meter` + `tray_blink` (5-4) compute RMS
//! over a sliding window and animate the tray icon at a rate
//! proportional to loudness. `transcriber` + `store` (5-5) run the lazy
//! whisper context and write JSON+WAV atomically. Tauri IPC for
//! `voice_start`/`voice_stop` lands in §5.7 (Part B).

pub mod abort;
pub mod capture;
pub mod hotkey;
pub mod meter;
pub mod model_manager;
pub mod permission;
pub mod store;
pub mod transcriber;
pub mod tray_blink;

pub use abort::{no_op_abort, voice_abort, AbortError};
pub use capture::{resample_to_16k, spawn_capture_loop, CaptureError, CaptureState, Frame};
pub use hotkey::{parse_hotkey, register as register_hotkey, HotKey, HotkeyError};
pub use meter::Meter;
pub use model_manager::{ensure_model, ensure_model_with, ModelError, EXPECTED_SHA256, MODEL_URL};
pub use permission::{
    check_mic_permission, mic_permission_deep_link_url, request_mic_permission, MicPermissionState,
};
pub use store::{
    delete as delete_voice_entry, new_entry_id, voice_paths, write_atomic, VoiceEntry,
};
pub use transcriber::{
    init_context as init_whisper_context, transcribe, Segment, TranscribeError, Transcript,
    WhisperContext,
};
pub use tray_blink::{BlinkLoop, IconCallback};
