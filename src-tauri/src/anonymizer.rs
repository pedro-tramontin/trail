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
//! * `anonymize` — MUST return a String containing the same logical
//!   content as `input`, with PII redacted. The output length is not
//!   bounded — the real impl may insert `[REDACTED:email]`-style
//!   tokens which can be longer than the matched substring. The
//!   shim (this v1) returns `input` byte-for-byte; the real impl
//!   rewrites matched substrings.
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
