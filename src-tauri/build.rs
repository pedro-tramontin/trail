//! Tauri 2 build script.
//!
//! 1. Hands off to `tauri_build::build()` for the standard Tauri codegen
//!    (icon/capabilities/permission registration).
//! 2. Copies the per-source collector schemas from the workspace's
//!    `crates/trail-collector/schemas/` directory into
//!    `src-tauri/resources/` so the bundled Tauri app ships every
//!    collector's JSON Schema verbatim. The supervisor in
//!    `crates/trail-collector/src/collect.rs` reads these schemas to
//!    validate raw-JSON output before write. Phase 2 §2.2 / §2.3 / §2.4.

use std::path::PathBuf;

fn main() {
    tauri_build::build();

    // Resolve the workspace root by walking up from CARGO_MANIFEST_DIR
    // (src-tauri/) until we find the `Cargo.toml` that has `[workspace]`.
    // Robust against build-hosts that nest the workspace in `..`.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let workspace_root = manifest_dir
        .ancestors()
        .find(|p| {
            let candidate = p.join("Cargo.toml");
            candidate.exists()
                && std::fs::read_to_string(&candidate)
                    .map(|c| c.contains("[workspace]"))
                    .unwrap_or(false)
        })
        .map(PathBuf::from)
        .expect("could not locate workspace root (no ancestor Cargo.toml has [workspace])");

    let schemas_dir = workspace_root.join("crates/trail-collector/schemas");
    let resources_dir = manifest_dir.join("resources");

    std::fs::create_dir_all(&resources_dir).expect("creating src-tauri/resources/");

    // Phase 2 §2.2 (github), §2.3 (claude_sessions), §2.4 (calendar). The
    // schemas land one item at a time — we copy whichever subset of the
    // three already exists in `schemas/` into the bundled resources. A
    // missing schema is a normal, expected state during Phase 2 in-flight:
    // the §2.3 and §2.4 items land after this one and each adds its
    // schema to the same directory. Build succeeds either way; the
    // supervisor's `compile_schema` call at runtime is what would fail if
    // a real collector was run before its schema landed.
    let per_source_schemas = [
        "github.schema.json",
        "claude_sessions.schema.json",
        "calendar.schema.json",
    ];
    for schema in per_source_schemas {
        let src = schemas_dir.join(schema);
        let dst = resources_dir.join(schema);
        if !src.exists() {
            println!(
                "cargo:warning=skipping schema copy (not yet present): {}",
                src.display()
            );
            continue;
        }
        std::fs::copy(&src, &dst)
            .unwrap_or_else(|e| panic!("copying {} → {}: {}", src.display(), dst.display(), e));
        println!("cargo:rerun-if-changed={}", src.display());
    }

    println!("cargo:rerun-if-changed={}", schemas_dir.display());
}
