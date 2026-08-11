//! `trail-collector` build script — enforces the musl-static contract.
//!
//! The collector ships to the VPS as a single, statically-linked binary
//! with zero runtime deps. The host where this script runs is usually
//! the developer's macOS laptop (Apple Silicon or Intel) or a Linux
//! build box; the binary that lands on the VPS is a `x86_64-unknown-
//! linux-musl` artifact.
//!
//! This script does **not** spawn a sub-`cargo build` for the musl
//! target (the parent plan does that on the laptop via
//! `cargo build --release -p trail-collector --target x86_64-unknown-linux-musl`
//! — the result lands in `target/x86_64-unknown-linux-musl/release/trail-collector`
//! and the install script copies it to the VPS). What the script DOES:
//!
//!   1. Re-run whenever the active target changes (`rerun-if-changed-env=TARGET`).
//!   2. Emit a `cargo:warning=` when the active target is not the musl
//!      target — i.e. when a developer runs `cargo build -p trail-collector`
//!      on their Linux host instead of cross-compiling for the VPS.
//!      The warning is a hint, not a hard error: the in-host build
//!      (used for the dev workflow + `cargo test`) still succeeds.
//!   3. Emit `cargo:rustc-link-arg=-static` so the link line picks
//!      up the C runtime statically when the musl target is active.
//!      On glibc hosts the `-static` flag is a no-op for Rust's own
//!      crate but still useful for the C deps the collector might pull
//!      in (currently none, but future-proof).
//!
//! **The actual musl cross-compile is NOT performed on this Linux build
//! host** — the musl target isn't installed in `/root/.rustup/toolchains/`
//! and we don't want to add it (it bloats the build env and the
//! production cross-compile only runs from the macOS developer laptop
//! per the master plan). The smoke test on this host is the plain
//! `cargo build --release -p trail-collector` (gnu target) — the
//! warning will fire, the build will still succeed, and the structure
//! of the build.rs is verified.

use std::env;

fn main() {
    // Re-run when the active target changes. (Cargo also re-runs when
    // any file in the workspace changes, but we want the build script
    // to re-evaluate even if a build is triggered with a different
    // --target flag and no other source changes.)
    println!("cargo:rerun-if-changed-env=TARGET");

    let target = env::var("TARGET").unwrap_or_default();
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();

    // The VPS-side target. Keep this in lock-step with the parent's
    // expectation; if you change one, change the other (the install
    // script + the `tauri build` pipeline both pin this string).
    const VPS_TARGET: &str = "x86_64-unknown-linux-musl";

    if target == VPS_TARGET {
        // Right target. Emit the static-link directive so the C
        // runtime is baked into the binary. Cargo forwards link-arg
        // additions verbatim to the linker.
        println!("cargo:rustc-link-arg=-static");
    } else if target_os == "macos" {
        // 2026-08-11 — when the collector is built as a macOS
        // binary (developer laptop or CI), link EventKit.framework
        // so the `objc2-event-kit` bindings can resolve
        // `EKEventStore` + the related C symbols at link time.
        // The `EventKit` link is a no-op on the Linux musl target
        // above (no macOS frameworks on Linux) so the line lives
        // only on this branch. The same line is added to
        // `src-tauri/build.rs` for the parent Tauri binary; the
        // collector and the parent each need their own because
        // they're separate crates and Cargo doesn't share build
        // script output between crates.
        println!("cargo:rustc-link-lib=framework=EventKit");
    } else {
        // Not the musl target — most likely the developer is running
        // `cargo build` on their own host (Linux gnu, macOS, etc.).
        // The build still succeeds (we don't fail the compile), but
        // the warning nudges them to use the right target for the
        // VPS ship.
        println!(
            "cargo:warning=trail-collector is being built for `{target}` \
             (os={target_os}, arch={target_arch}), not `{VPS_TARGET}`. \
             The artifact will NOT be deployable to the VPS as-is. \
             To ship a static binary, cross-compile with:\n  \
             cargo build --release -p trail-collector --target {VPS_TARGET}\n\
             The actual cross-compile runs on the macOS developer laptop \
             (see docs/install.md); the on-host build is for dev/test only."
        );
    }
}
