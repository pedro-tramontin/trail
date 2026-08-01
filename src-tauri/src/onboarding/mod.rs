//! Phase 6 onboarding scaffolding. The two submodules are:
//!
//! - [`scan`]: non-invasive laptop filesystem + env probe. Detects which
//!   collectors the user *could* install based on artifacts already on
//!   the disk. Never reads, never installs.
//!
//! Later phases (6-2 LLM Q&A, 6-3 config writer) live alongside this
//! module once the spec lands. Keeping this module root small so the
//! diff for 6-1 stays focused on the scan surface.

pub mod scan;

pub use scan::{CollectorCandidate, CollectorStatus, EvidenceKind, Platform, ScanReport};
