//! Voice capture pipeline (Phase 5).
//!
//! `model_manager` (5-1) downloads + verifies the whisper GGML file the
//! first time any voice feature needs it. Phase 5 will add `recorder`
//! (5-2 mic capture via `hound`), `transcriber` (5-3 whisper-rs
//! bindings), `hotkey` (5-4 global shortcut), and `commands` (5-5 IPC).

pub mod model_manager;

pub use model_manager::{ensure_model, ensure_model_with, ModelError, EXPECTED_SHA256, MODEL_URL};
