//! Voice capture pipeline (Phase 5).
//!
//! `model_manager` (5-1) downloads + verifies the whisper GGML file the
//! first time any voice feature needs it. `capture` (5-2) opens the
//! microphone via cpal on macOS and resamples to 16 kHz mono via
//! rubato. `hotkey` (5-3) parses the push-to-talk shortcut string
//! (`Ctrl+Shift+Space` by default) and registers it on macOS with
//! conflict detection. Upcoming: `transcriber` (5-4 whisper-rs
//! bindings) and `commands` (5-5 IPC).

pub mod capture;
pub mod hotkey;
pub mod model_manager;

pub use capture::{resample_to_16k, spawn_capture_loop, CaptureError, Frame};
pub use hotkey::{parse_hotkey, register as register_hotkey, HotKey, HotkeyError};
pub use model_manager::{ensure_model, ensure_model_with, ModelError, EXPECTED_SHA256, MODEL_URL};
