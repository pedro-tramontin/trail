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
    trail_lib::run()
}
