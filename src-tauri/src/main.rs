// Demo mode bootstrap handoff: the `Args` struct below is parsed by
// `clap`; on `--demo` we set `TRAIL_DEMO=1` so `lib::run()` (which
// doesn't take args) can read the flag and pass it to
// `demo::activate_if_requested`. The env-var handoff is a deliberate
// choice for a single boolean — see the Phase 7 plan §7.5 "Heads-up"
// note for the future migration path if we add more demo flags.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use clap::Parser;

/// CLI args for the Trail menu-bar app.
///
/// Currently just the `--demo` first-run flag, which boots the app
/// with fixture data + a yellow banner (instead of a real
/// `~/.trail/config.json` + SSH transport) so a new visitor can
/// poke the UI without setting up the full stack. When `--demo` is
/// passed but a real config already exists, demo mode is NOT
/// activated (the bootstrap check in `demo::activate_if_requested`
/// refuses to clobber a real install).
#[derive(Parser, Debug)]
#[command(
    name = "trail",
    version,
    about = "Trail menu-bar app — passive workday capture + VPS push."
)]
struct Args {
    /// Start the app in demo mode: fixture data, no SSH push, banner
    /// at the top of every window. Use this for first-run exploration
    /// without setting up the full stack (no config, no VPS, no SSH
    /// key). If a real `~/.trail/config.json` already exists, this
    /// flag is ignored — you must delete the config first.
    #[arg(long)]
    demo: bool,
}

fn main() {
    let args = Args::parse();
    if args.demo {
        // Stash the demo flag so `lib::run()` can pick it up.
        std::env::set_var("TRAIL_DEMO", "1");
    }

    // Workaround for a Tahoe (macOS 26.5.2) `AVFCore` regression that
    // crashed `trail` (process PID 13043, Incident
    // 8A4EA1EC-9550-4340-8207-CDDDB0146840) with `EXC_BAD_ACCESS
    // (SIGBUS)` at `+[AVCaptureDevice authorizationStatusForMediaType:]`
    // when the Svelte onboarding wizard transitioned from step 2 to
    // step 3 and invoked the `check_mic_permission` Tauri command.
    //
    // Root cause: on first touch, the ObjC runtime writes class
    // metadata for `AVCaptureDevice_Tundra` into the AVFCore
    // `__AUTH_CONST` segment; on Tahoe that page is mapped
    // `r--/rw- SM=COW` but the kernel refuses the write with
    // `KERN_PROTECTION_FAILURE`. Doing the first-touch here on the
    // main thread, before the Tauri runtime / webview is up, lets
    // the objc runtime pick a writable page for the metadata once at
    // startup; subsequent calls from the `check_mic_permission` IPC
    // command are no-op realizations and never touch the
    // write-protected page again.
    //
    // Once Apple ships an `AVFCore` update that moves the affected
    // metadata out of `__AUTH_CONST` (or Tahoe's COW handling is
    // fixed), this block can be deleted — `check_mic_permission`'s
    // first call from the IPC handler will succeed without it.
    #[cfg(target_os = "macos")]
    {
        use objc2::{class, msg_send};
        extern "C" {
            #[link_name = "AVMediaTypeAudio"]
            static AVMediaTypeAudio: objc2::runtime::AnyObject;
        }
        // SAFETY: `class!(AVCaptureDevice)` returns the metaclass
        // registered by AVFoundation, `AVMediaTypeAudio` is a
        // read-only NSString constant exported by the framework, and
        // `authorizationStatusForMediaType:` is a pure (no side
        // effects beyond a TCC read) class method. The whole point
        // of this call is to trigger the first-touch realization so
        // any subsequent call from the IPC layer is a no-op.
        let _: isize = unsafe {
            msg_send![
                class!(AVCaptureDevice),
                authorizationStatusForMediaType: &AVMediaTypeAudio
            ]
        };
    }

    trail_lib::run()
}
