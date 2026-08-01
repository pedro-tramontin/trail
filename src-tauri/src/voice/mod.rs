//! Voice capture pipeline (Phase 5).
//!
//! `model_manager` (5-1) downloads + verifies the whisper GGML file the
//! first time any voice feature needs it. `capture` (5-2) opens the
//! microphone via cpal on macOS and resamples to 16 kHz mono via
//! rubato. `hotkey` (5-3) parses the push-to-talk shortcut string
//! (`Ctrl+Shift+Space` by default) and registers it on macOS with
//! conflict detection. `meter` + `tray_blink` (5-4) compute RMS over
//! a sliding window and animate the tray icon at a rate proportional
//! to loudness. Upcoming: `transcriber` (whisper-rs bindings) and
//! `commands` (5-5 IPC).

pub mod capture;
pub mod hotkey;
pub mod meter;
pub mod model_manager;
pub mod tray_blink;

pub use capture::{resample_to_16k, spawn_capture_loop, CaptureError, Frame};
pub use hotkey::{parse_hotkey, register as register_hotkey, HotKey, HotkeyError};
pub use meter::Meter;
pub use model_manager::{ensure_model, ensure_model_with, ModelError, EXPECTED_SHA256, MODEL_URL};
pub use tray_blink::{BlinkLoop, IconCallback};
