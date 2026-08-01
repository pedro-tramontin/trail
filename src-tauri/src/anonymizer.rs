//! Phase 3 anonymizer — SHIM for item 3-2. Real impl lands in item 3-3.
//!
//! Currently a no-op pass-through that returns `input` unchanged. Item 3-3
//! will replace this with the regex scrubber that redacts emails,
//! phone numbers, GitHub usernames, calendar attendees, etc. with strictness
//! tiers (Off / Moderate / Aggressive) gated on the user's
//! `Config.summarizer.anonymization_strictness` value.
//!
//! The shim exists so item 3-2 (summarizer) can compile today and call
//! `anonymize(&text, strictness, &rules)` without a temporary todo in
//! `summarizer::run` waiting for the real impl. The contract is:
//!
//! * `anonymize` — MUST return a String of the same length-or-shorter as
//!   `input`. In v1 (no-op) it returns `input` byte-for-byte. Once 3-3
//!   lands, the scrubber may insert `[REDACTED:email]` tokens.
//! * `strictness` — `"off"`, `"moderate"`, or `"aggressive"`. The shim
//!   ignores it; the real impl branches on the value.
//! * `rules` — per-user custom rules (e.g. "redact Acme Corp"). Empty in
//!   v1; the shim ignores. The real impl iterates + applies each regex.

/// Anonymization level. Mirrors the string values written into
/// `Config.summarizer.anonymization_strictness` (`"off"`, `"moderate"`,
/// `"aggressive"`). Kept as a separate type from the string the caller
/// passes in so 3-3 can switch to typed matching without an API break.
#[allow(dead_code)]
pub enum AnonymizationStrictness {
    Off,
    Moderate,
    Aggressive,
}

/// No-op pass-through. Item 3-3 replaces this with the regex scrubber.
#[allow(dead_code)]
pub fn anonymize(input: &str, _strictness: &str, _rules: &[String]) -> String {
    input.to_string()
}
