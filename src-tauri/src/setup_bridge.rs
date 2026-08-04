//! Phase 9 §9.1 — bridge between the Tauri runtime and
//! `crate::start_collectors_inner`. The command wrapper lives here
//! rather than at the crate root so that `tauri-macros` 2.6.3's
//! `#[macro_export]` on `pub`-visible command functions doesn't
//! collide at the crate root (where the macro_rules! definition AND
//! the visibility-scoped `use` re-export would otherwise land at
//! the same path, triggering an `E0255` "defined multiple times"
//! on `__cmd__start_collectors`).
//!
//! Putting the command in its own module scopes both the macro
//! definition and the macro_export re-export to that module.
//! `crate::start_collectors` at the lib.rs root is a thin proxy
//! that calls into here; the `invoke_handler!` macro list picks
//! the lib-root proxy so the IPC contract name stays
//! "start_collectors".
//!
//! Phase 9 §9.3 — also hosts the testable `build_initial_window`
//! helper that the `lib.rs` setup closure calls to open either
//! the `main` shell (when a config already exists on disk) or the
//! `onboarding` wizard window (when it doesn't). Factored out into
//! a generic `pub fn` so the §9.3 integration test
//! (`tests/headless_launch.rs::headless_launch_opens_onboarding_window_when_no_config`)
//! can assert the right window is built for each `ConfigState`
//! without dragging in the Tauri 2.11.5 runtime (whose
//! `mock_builder().build()` shim does NOT run the setup closure
//! synchronously — see the §9.1 D2 / §9.2 D11 lessons in state.md
//! and the file-level doc on `tests/headless_launch.rs`).

use std::sync::Arc;
use tauri::Manager;

/// Tauri command wrapper. Resolves the config path via the
/// platform-correct `AppHandle`, calls `start_collectors_inner`
/// to bring up the orchestrator + scheduler, then flips the
/// `ConfigState` to `Ready(cfg)` so subsequent IPC commands see a
/// live orchestrator.
#[tauri::command]
pub fn start_collectors(app: tauri::AppHandle) -> Result<(), String> {
    let dir = app
        .path()
        .app_config_dir()
        .map_err(|e| format!("app_config_dir: {e}"))?;
    let config_path = dir.join("config.json");
    let (orch, sched_task) = crate::start_collectors_inner(&config_path)?;
    // Re-load to capture the parsed config + flip the state machine
    // from `AwaitingOnboarding` to `Ready(...)` so subsequent IPC
    // commands see a live orchestrator.
    let cfg = crate::config::load_config(&config_path)
        .map_err(|e| format!("loading config from {}: {e}", config_path.display()))?;
    app.manage(orch);
    app.manage(Arc::new(sched_task));
    app.manage(crate::ConfigState::Ready(Box::new(cfg)));
    Ok(())
}

/// Phase 9 §9.3 — descriptor for which window the setup closure
/// should open at first boot. The actual `WebviewWindowBuilder`
/// call needs a live `AppHandle` + a registered `tauri.conf.json`
/// `windows` entry (the §9.3 fallback), so the test asserts the
/// descriptor logic here rather than the builder call. See
/// `window_descriptor_for` for the pure helper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitialWindowDescriptor {
    /// The `WebviewWindow` label (`"main"` or `"onboarding"`).
    pub label: &'static str,
    /// The URL path the webview loads — `"index.html"` for the
    /// main shell, `"index.html?wizard=1"` for the wizard (the
    /// `?wizard=1` query param is what the frontend's router uses
    /// to auto-mount the `Onboarding.svelte` wizard regardless of
    /// the probe result, as a belt-and-suspenders for the
    /// cold-restart case).
    pub url: &'static str,
    /// Whether the window should start visible. Always `true`
    /// after the §9.3 plumbing is wired (the `tauri.conf.json`
    /// fallback sets `visible: false` so the runtime doesn't show
    /// a default empty window before the setup closure runs; the
    /// setup closure then explicitly shows whichever window the
    /// config state requires).
    pub visible: bool,
}

/// Phase 9 §9.3 — pure helper. Maps a `ConfigState` to the window
/// the setup closure should open. `pub` so the integration test
/// can assert the logic without instantiating a `WebviewWindow`.
///
/// Contract:
/// - `ConfigState::Ready(_)`     → open the `main` shell at
///   `index.html` (the regular app).
/// - `ConfigState::AwaitingOnboarding` → open the `onboarding`
///   wizard at `index.html?wizard=1`.
pub fn window_descriptor_for(state: &crate::ConfigState) -> InitialWindowDescriptor {
    match state {
        crate::ConfigState::Ready(_) => InitialWindowDescriptor {
            label: "main",
            url: "index.html",
            visible: true,
        },
        crate::ConfigState::AwaitingOnboarding => InitialWindowDescriptor {
            label: "onboarding",
            url: "index.html?wizard=1",
            visible: true,
        },
    }
}

/// Phase 9 §9.3 — build the initial webview window from the
/// `ConfigState` the setup closure just registered. Called once
/// per boot, immediately after the §9.2 tray-icon build.
///
/// The `WebviewWindowBuilder` call needs a live `AppHandle`, so
/// this function is generic over `tauri::Runtime` and isn't
/// directly unit-testable in the Linux CI (no
/// `mock_builder().build()`-synchronous setup closure in Tauri
/// 2.11.5). The integration test in `tests/headless_launch.rs`
/// asserts the logic via `window_descriptor_for` above (a pure
/// function on `ConfigState`); this `build_initial_window` exists
/// to be the one place the actual builder call lives, so the
/// setup closure doesn't have to inline the `match`.
///
/// Errors propagate via `?` (e.g. WebviewWindow construction
/// failures, missing icon, etc.) — the setup closure's `Box<dyn
/// std::error::Error>` return type handles them.
pub fn build_initial_window<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    state: &crate::ConfigState,
) -> tauri::Result<tauri::WebviewWindow<R>> {
    let descriptor = window_descriptor_for(state);
    tracing::info!(
        "opening initial window label={} url={} (config_present={})",
        descriptor.label,
        descriptor.url,
        matches!(state, crate::ConfigState::Ready(_))
    );
    tauri::WebviewWindowBuilder::new(
        app,
        descriptor.label,
        tauri::WebviewUrl::App(descriptor.url.into()),
    )
    .title(if descriptor.label == "main" {
        "Trail"
    } else {
        "Trail — Onboarding"
    })
    .inner_size(
        if descriptor.label == "main" {
            900.0
        } else {
            720.0
        },
        if descriptor.label == "main" {
            700.0
        } else {
            560.0
        },
    )
    .visible(descriptor.visible)
    .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ConfigState;

    /// §9.3 — when a config is already on disk (`Ready`), the
    /// setup closure MUST open the `main` shell (the regular
    /// app), not the wizard. Opening the wizard when a config
    /// already exists would re-prompt the user to re-onboard
    /// every cold restart.
    #[test]
    fn window_descriptor_for_ready_opens_main_shell() {
        // Build a real `Config` by writing a minimal JSON to a
        // tempdir and loading it via `config::load_config` (the
        // same call path the production setup closure uses).
        // `Config` doesn't impl `Default` (every field is
        // required by the v1 schema), so this is the only way
        // to obtain a `ConfigState::Ready(_)` in a test without
        // a hand-rolled fixture crate.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg_path = tmp.path().join("config.json");
        let minimal_config = r#"{
            "claude_sessions_paths": [],
            "github": {"mode": "gh_cli", "host": "github.com"},
            "calendar_ics": "/nonexistent.ics",
            "voice": {"enabled": true, "hotkey": "ctrl+shift+space", "transcriber": "whisper_cpp", "model": "base.en"},
            "review_time": "18:00",
            "summarizer": {"model": "gpt-oss:20b", "model_provider": "local", "anonymization_strictness": "aggressive", "use_generic_categories": true},
            "transport": {"type": "ssh", "host": "vm.example.com", "port": 22, "user": "trail", "auth": {"auth": "public_key", "path": "/tmp/trail-test-key"}, "remote_path": "/tmp/trail-remote"},
            "raw_retention_days": 7,
            "pending_installs": []
        }"#;
        std::fs::write(&cfg_path, minimal_config).expect("write minimal config");
        let cfg = crate::config::load_config(&cfg_path).expect("load minimal config");
        let descriptor = window_descriptor_for(&ConfigState::Ready(Box::new(cfg)));
        assert_eq!(
            descriptor.label, "main",
            "Ready config must open the 'main' shell"
        );
        assert_eq!(
            descriptor.url, "index.html",
            "main shell must load index.html (not the wizard query)"
        );
        assert!(
            descriptor.visible,
            "main shell must start visible after the setup closure decides to show it"
        );
    }

    /// §9.3 — when no config is on disk (`AwaitingOnboarding`),
    /// the setup closure MUST open the `onboarding` wizard, not
    /// the main shell. Opening the main shell on first launch
    /// would show a blank Tauri webview with no path to the
    /// wizard — the user would have no way to configure the
    /// collector.
    #[test]
    fn window_descriptor_for_awaiting_onboarding_opens_wizard() {
        let descriptor = window_descriptor_for(&ConfigState::AwaitingOnboarding);
        assert_eq!(
            descriptor.label, "onboarding",
            "AwaitingOnboarding must open the 'onboarding' wizard window"
        );
        assert!(
            descriptor.url.contains("wizard=1"),
            "wizard window URL must include the ?wizard=1 query param so the frontend auto-mounts Onboarding.svelte; got {}",
            descriptor.url
        );
        assert!(
            descriptor.visible,
            "wizard window must start visible after the setup closure decides to show it"
        );
    }
}
