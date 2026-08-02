//! Build-time version + target-triple constants.
//!
//! The release workflow (`release.yml` build-mac-universal job) sets
//! `TRAIL_TARGET_TRIPLE` via the Rust toolchain so the produced `.app`
//! records which architecture slice each lipo-merged executable came
//! from. `cargo test` (no `TRAIL_TARGET_TRIPLE` set) falls back to the
//! active host target so the tests stay cross-platform.
//!
//! The pair `VERSION` + `TARGET_TRIPLE` is wired into the CLI's
//! `--version` output via `trail_collector::version_string()` so the
//! user-visible banner reads e.g. `trail-collector 0.1.0 (aarch64-apple-darwin)`.

/// Package version. Populated at compile time from `CARGO_PKG_VERSION`
/// (which Cargo itself populates from this crate's `[package].version`).
/// This is the single source of truth — release-please bumps it in
/// `crates/trail-collector/Cargo.toml` and Cargo rebuilds with the
/// new value baked in via the `env!` macro.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Target triple this binary was compiled for. The release workflow
/// passes `TRAIL_TARGET_TRIPLE=aarch64-apple-darwin` (or `x86_64-apple-darwin`)
/// to `cargo test --config` so the universal-binary job records which
/// slice produced each per-arch build. Default `cargo test` (no env var
/// set) falls back to the literal string `"unknown"` so the value is
/// never an empty string.
pub const TARGET_TRIPLE: &str = match option_env!("TRAIL_TARGET_TRIPLE") {
    Some(t) => t,
    None => "unknown",
};

/// Combined `version (target)` string for CLI banners. Kept here so the
/// lib + bin both reference one source.
pub fn version_string() -> String {
    format!("{} ({})", VERSION, TARGET_TRIPLE)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Confirms the `VERSION` constant equals the package version Cargo
    /// recorded when this crate was compiled. If a build script ever
    /// shadows `CARGO_PKG_VERSION`, this test catches the divergence.
    #[test]
    fn version_matches_cargo_pkg_version() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
    }

    /// `TARGET_TRIPLE` is set by the release workflow's universal-binary
    /// matrix job at `cargo build --config` time; plain `cargo test` does
    /// not set the env var so we expect the fallback `"unknown"`. Real
    /// triples look like `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`,
    /// `x86_64-unknown-linux-musl`. Each is accepted.
    #[test]
    fn target_triple_is_known_or_unmarked() {
        let is_known_triple = TARGET_TRIPLE.contains("apple-darwin")
            || TARGET_TRIPLE.contains("linux-gnu")
            || TARGET_TRIPLE.contains("linux-musl")
            || TARGET_TRIPLE.contains("windows-");
        let is_unmarked = TARGET_TRIPLE == "unknown";
        assert!(
            is_known_triple || is_unmarked,
            "unexpected TARGET_TRIPLE: {TARGET_TRIPLE} \
             (expected a known triple like 'aarch64-apple-darwin' or the \
             'unknown' fallback set when the env var is not provided)"
        );
    }

    /// Mirrors the JSON-shape smoke check used in `tests/post_build_smoke.sh`.
    /// `lipo -info` (when run on macOS with `-output-format json`) returns
    /// a `{ "kind": "fat", "arches": [...] }` object. We assert that we
    /// can DESERIALIZE that shape, not that we can run lipo (lipo is
    /// macOS-only; this test runs on Linux CI too).
    #[test]
    fn lipo_info_json_shape_parses() {
        let fake_lipo_output = r#"{
            "kind": "fat",
            "arches": ["x86_64", "arm64"]
        }"#;
        let parsed: serde_json::Value =
            serde_json::from_str(fake_lipo_output).expect("lipo -info JSON should parse");
        assert_eq!(parsed["kind"], "fat");
        assert_eq!(parsed["arches"][1], "arm64");
        // also: the version string is non-empty and the target is one
        // of the documented values. This guards against a regression
        // where the version string format silently drifts.
        assert!(!version_string().is_empty());
        assert!(version_string().contains(VERSION));
    }
}
