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

    // Phase 5 §5.7 — link AVFoundation (and its two transitive
    // deps that expose the AVMediaTypeAudio extern NSString* + the
    // audio session primitives that `AVCaptureDevice` calls into)
    // into the main binary on macOS. `objc2::class!(AVCaptureDevice)`
    // + `AVMediaTypeAudio` symbol lookup need the framework to be
    // resolvable at link time; gating to macOS keeps the Linux build
    // (used in CI) free of `-framework AVFoundation` flags which the
    // Linux linker wouldn't accept.
    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=AVFoundation");
        println!("cargo:rustc-link-lib=framework=CoreMedia");
        println!("cargo:rustc-link-lib=framework=AudioToolbox");
        // 2026-08-11 — EventKit for the calendar collector. The
        // `EKEventStore` class lives in `EventKit.framework`; the
        // macOS build of `trail-collector` resolves the same
        // framework link (the binary inherits the link list from
        // the parent crate's build.rs because we `link` it from
        // `src-tauri/Cargo.toml`'s target-conditional deps).
        println!("cargo:rustc-link-lib=framework=EventKit");
    }

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
