//! Phase 6 onboarding scaffolding. Submodules:
//!
//! - [`scan`]: non-invasive laptop filesystem + env probe. Detects which
//!   collectors the user *could* install based on artifacts already on
//!   the disk. Never reads, never installs.
//! - [`answers`]: the typed `OnboardingAnswers` struct that Phase C
//!   (config-writer) consumes, plus the LLM-envelope shape that mirrors
//!   `schemas/onboarding-answer.schema.json`.
//! - [`baseline`]: the hardcoded fallback answers when ollama is
//!   unreachable. Pure data transform over a `ScanReport`.
//! - [`llm`]: the LLM-driven entry point (`ask_onboarding`) that feeds
//!   the scan to a local ollama server with structured output, validates
//!   the response against the JSON Schema, and flattens the envelope
//!   into the typed `OnboardingAnswers`. Falls back to [`baseline`]
//!   on any ollama failure.
//! - [`config_writer`]: Phase C — converts [`OnboardingAnswers`] into
//!   the frozen `Config`, atomically writes it to `~/.trail/config.json`,
//!   appends a JSONL audit log, and one-shots the legacy
//!   `~/.workday-logger/config.json` migration.

pub mod answers;
pub mod baseline;
pub mod config_writer;
pub mod event_kit;
pub mod llm;
pub mod scan;

pub use answers::OnboardingAnswers;
pub use config_writer::write_config;
pub use scan::{CollectorCandidate, CollectorStatus, EvidenceKind, Platform, ScanReport};
