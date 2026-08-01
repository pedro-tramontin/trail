//! Voice capture pipeline (Phase 5).
//!
//! `model_manager` (5-1) downloads + verifies the whisper GGML file the
//! first time any voice feature needs it. `capture` (5-2) opens the
//! microphone via cpal on macOS and resamples to 16 kHz mono via
//! rubato. Upcoming: `transcriber` (5-3 whisper-rs bindings), `hotkey`
//! (5-4 global shortcut), and `commands` (5-5 IPC).

pub mod capture;
pub mod model_manager;

pub use capture::{resample_to_16k, spawn_capture_loop, CaptureError, Frame};
pub use model_manager::{ensure_model, ensure_model_with, ModelError, EXPECTED_SHA256, MODEL_URL};
