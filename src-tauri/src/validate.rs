//! Client-side pre-push validation for day-summary payloads.
//!
//! Mirrors the collector's `crates/trail-collector/src/validate.rs` (item
//! 1-5) but lives in the Tauri app so the user-facing wizard can catch
//! schema errors BEFORE the `push_to_vps` Tauri command is invoked.
//! Both sides validate against the SAME bundled schema file
//! (`resources/day-summary.schema.json` at the workspace root), so a
//! payload that passes client-side validation also passes collector-side
//! validation when it's picked up by the cron job.
//!
//! **Schema loading: compile-time `include_str!`** (not runtime
//! `app.path().resource_dir()`). The trade-off:
//!
//! - `include_str!` bakes the schema into the binary at compile time.
//!   Pro: the validator can't drift from the compiled-in schema (no
//!   risk of a stale bundled resource file). Con: schema changes
//!   require a rebuild.
//! - `app.path().resource_dir()` reads the schema at runtime from
//!   the Tauri bundle. Pro: schema can be updated by replacing the
//!   bundled file. Con: requires the `tauri.conf.json`
//!   `bundle.resources` mapping (which is already in place from
//!   item 1-1) to be in sync; otherwise the validator reads a
//!   different file than the bundled one.
//!
//! We pick `include_str!` because the schema is part of the build
//! contract (item 1-5's master plan calls it a "frozen contract" and
//! any change is a coordinated v2 bump, not a runtime swap). If a
//! future need arises to swap the schema without rebuilding, the
//! runtime load can be added as a fallback; the compile-time path is
//! the source of truth.
//!
//! **All errors at once, not short-circuit.** The user-facing UX is
//! better when they see "missing fields: [date, summary]" instead of
//! "missing field: date; re-submit; missing field: summary; re-submit…".
//! `jsonschema::JSONSchema::validate` returns an iterator over all
//! validation errors; we collect + sort them before returning.

use jsonschema::JSONSchema;
use serde::Serialize;

/// All errors from a single validation pass, sorted for stable
/// display. The shape is serializable so a future Tauri command can
/// surface the list to the frontend (today the Tauri command only
/// returns the list as `String` for compatibility with the existing
/// `Result<_, String>` shape, but the structured form is kept for
/// future-proofing).
#[derive(Debug, Serialize, PartialEq)]
pub struct ValidateError {
    /// Human-readable error messages, one per schema violation.
    /// Sorted alphabetically so the output is stable across runs
    /// (the schema validator's iteration order is not guaranteed).
    pub errors: Vec<String>,
}

impl std::fmt::Display for ValidateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "validation failed: {}", self.errors.join("; "))
    }
}

impl std::error::Error for ValidateError {}

/// The bundled day-summary schema, embedded at compile time. The
/// path is relative to this source file (`src-tauri/src/validate.rs`),
/// so `../resources/day-summary.schema.json` resolves to
/// `<workspace-root>/resources/day-summary.schema.json` — the SAME
/// file the collector's `--validate` / `--once` modes read at
/// runtime on the VPS. Both sides validate against the same
/// bytes.
const SCHEMA_TEXT: &str = include_str!("../../resources/day-summary.schema.json");

/// Compile the bundled schema. `OnceLock` makes the compile-once
/// guarantee: the first call parses + compiles the schema, every
/// subsequent call returns the same compiled schema without
/// re-doing the (non-trivial) JSON Schema compilation work.
///
/// `jsonschema::JSONSchema::compile` requires the schema to be
/// `Serialize` + `Send + Sync + 'static`; `serde_json::Value`
/// satisfies all of these. The resulting `JSONSchema` is
/// immutable for the lifetime of the program.
fn compiled_schema() -> &'static JSONSchema {
    use std::sync::OnceLock;
    static SCHEMA: OnceLock<JSONSchema> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        let schema_value: serde_json::Value = serde_json::from_str(SCHEMA_TEXT)
            .expect("bundled day-summary.schema.json is valid JSON");
        JSONSchema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .compile(&schema_value)
            .expect("bundled day-summary.schema.json is a valid Draft 2020-12 schema")
    })
}

/// Validate `payload` against the bundled day-summary schema.
///
/// Returns `Ok(())` if the payload conforms, or `Err(ValidateError)`
/// containing ALL the schema violations (sorted, deduplicated) if
/// it doesn't. Errors are not short-circuited — the caller (the
/// `validate_day_summary` Tauri command) gets the full picture in
/// one round trip.
pub fn validate(payload: &serde_json::Value) -> Result<(), ValidateError> {
    let result = compiled_schema().validate(payload);
    match result {
        Ok(()) => Ok(()),
        Err(errors) => {
            // Collect + dedupe + sort so the output is stable.
            let mut msgs: Vec<String> = errors
                .map(|e| format!("{} at {}", e, e.instance_path))
                .collect();
            msgs.sort();
            msgs.dedup();
            Err(ValidateError { errors: msgs })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // Mirror of the bundled schema (item 1-5's master contract).
    // Used in tests to assert the validate() function behaves the
    // way the schema says it should. If the schema changes, the
    // tests should be updated to match.

    #[test]
    fn validate_accepts_valid_day_summary() {
        let payload = json!({
            "date": "2026-07-31",
            "summary": "Worked on the trail design.",
            "wins": ["Phase D parameterized", "Schema frozen"],
            "blockers": ["Tauri 2 on headless host"],
            "people": ["colleague-A"],
            "open_threads": ["v2 plugin system"],
            "voice_notes": []
        });
        let result = validate(&payload);
        assert!(result.is_ok(), "expected Ok, got {:?}", result);
    }

    #[test]
    fn validate_rejects_missing_required_field() {
        // Drop the `blockers` field. The schema has `additionalProperties: false`,
        // so dropping a required field produces a "required" violation.
        let payload = json!({
            "date": "2026-07-31",
            "summary": "x",
            "wins": [],
            "people": [],
            "open_threads": [],
            "voice_notes": []
        });
        let err = validate(&payload).expect_err("expected validation error for missing field");
        assert!(!err.errors.is_empty(), "error list must not be empty");
        // The error should mention the missing key (`blockers`) or the
        // `required` keyword. We don't pin the exact message (jsonschema
        // crate's wording drifts) but we do assert the error surfaced.
        let joined = err.errors.join(" | ");
        assert!(
            joined.contains("blockers") || joined.to_lowercase().contains("required"),
            "expected error to mention 'blockers' or 'required', got: {joined:?}"
        );
    }

    #[test]
    fn validate_rejects_extra_field() {
        // `additionalProperties: false` means a stray key is a violation.
        let payload = json!({
            "date": "2026-07-31",
            "summary": "x",
            "wins": [],
            "blockers": [],
            "people": [],
            "open_threads": [],
            "voice_notes": [],
            "sneaky_extra_field": "not in the schema"
        });
        let err = validate(&payload).expect_err("expected error for extra field");
        let joined = err.errors.join(" | ");
        assert!(
            joined.contains("sneaky_extra_field")
                || joined.to_lowercase().contains("additional")
                || joined.to_lowercase().contains("unknown"),
            "expected error to mention the extra field or 'additional'/'unknown', got: {joined:?}"
        );
    }

    #[test]
    fn validate_rejects_malformed_date() {
        // The schema's `date` field is `pattern: ^\d{4}-\d{2}-\d{2}$`.
        // "31-07-2026" is DD-MM-YYYY, not the ISO 8601 the schema requires.
        let payload = json!({
            "date": "31-07-2026",
            "summary": "x",
            "wins": [],
            "blockers": [],
            "people": [],
            "open_threads": [],
            "voice_notes": []
        });
        let err = validate(&payload).expect_err("expected error for malformed date");
        let joined = err.errors.join(" | ");
        // The exact error wording varies across jsonschema versions;
        // assert the error mentions the date field or a pattern violation.
        assert!(
            joined.contains("date") || joined.to_lowercase().contains("pattern"),
            "expected error to mention 'date' or 'pattern', got: {joined:?}"
        );
    }
}
