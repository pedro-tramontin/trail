//! Phase 9 §9.2 — bridge module hosting the `show_main_window` Tauri
//! command.
//!
//! Same rationale as `setup_bridge.rs`: `tauri-macros` 2.6.3's
//! `#[macro_export]` on `pub` command functions doesn't collide at
//! the crate root if the command lives in a sub-module. The
//! `#[tauri::command]` definition AND the macro_export re-export
//! both scope to this module's path, avoiding the `E0255`
//! "defined multiple times" error on `__cmd__show_main_window`
//! at the lib.rs root. `crate::show_main_window` at lib.rs is a
//! thin proxy that calls into here; `generate_handler!` references
//! the lib-root proxy so the IPC contract name stays
//! `show_main_window`.
//!
//! Also hosts the [`MAIN_TRAY_MENU_ITEMS`] constant — the
//! `(id, label)` pairs the tray-icon's right-click menu surfaces
//! ("Show Trail" + "Quit Trail" in v1). Exposed as a `pub` static
//! so the §9.2 integration test in `tests/headless_launch.rs` can
//! assert the menu content without needing a live Tauri runtime
//! (the `tauri::test::mock_builder().build()` path does NOT run
//! the setup closure in 2.11.5, so the test can't reach the actual
//! `tauri::menu::Menu` build site — see the §9.1 lessons in
//! state.md and the doc comment on `headless_launch_tray_icon_is_built`).

use tauri::Manager;

/// One tray menu entry, in render order. The `id` is the
/// `tauri::menu::MenuItem` id that the `on_menu_event` closure
/// matches on; the `label` is the human-readable text the menu
/// shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MainTrayMenuItem {
    /// The `tauri::menu::MenuItem` id (matched in the
    /// `on_menu_event` closure).
    pub id: &'static str,
    /// The user-visible label.
    pub label: &'static str,
}

/// The menu items the main tray icon renders (v1 surface — just
/// "Show Trail" + "Quit Trail"). Render order = slice order. The
/// "Start recording" / "Stop recording" / "Open mic settings"
/// items from the §5.7 dead-code scaffold are deferred to a
/// Phase 10 follow-up per Decision 1 in the plan.
///
/// Exposed as a `pub` static so the integration test can assert
/// the menu content without a live Tauri runtime.
pub static MAIN_TRAY_MENU_ITEMS: &[MainTrayMenuItem] = &[
    MainTrayMenuItem {
        id: "show",
        label: "Show Trail",
    },
    MainTrayMenuItem {
        id: "quit",
        label: "Quit Trail",
    },
];

/// Tauri command: show the main window.
///
/// Phase 9 §9.2 — wired to the tray-icon's "Show Trail" menu item
/// and the `on_tray_icon_event` left-click handler in `lib.rs`'s
/// setup closure. Also useful for the wizard's "Open settings"
/// button (the wizard's own webview window is the one that's
/// open at the moment, not the main shell — so clicking that
/// button after the wizard finishes will surface the main shell).
///
/// If no webview window with the label `"main"` exists yet (very
/// early boot, before §9.3 wires the main window), this is a
/// silent no-op rather than an error. The tray-icon "Show Trail"
/// click is then a no-op too — the user just sees no window
/// appear, which is the same UX as if the binary hadn't started
/// yet.
#[tauri::command]
pub fn show_main_window(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        win.show().map_err(|e| format!("show: {e}"))?;
        win.set_focus().map_err(|e| format!("set_focus: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The main tray menu MUST include "Show Trail" as the first
    /// item — the user needs an entry point to surface the main
    /// window from the menu-bar icon. This is the §9.2 visibility
    /// contract: if the "show" id is missing, the binary has no
    /// way to bring up the main window on macOS.
    #[test]
    fn main_tray_menu_starts_with_show_trail() {
        assert!(
            !MAIN_TRAY_MENU_ITEMS.is_empty(),
            "main tray menu must have at least one item"
        );
        let first = MAIN_TRAY_MENU_ITEMS[0];
        assert_eq!(first.id, "show", "first menu item id must be 'show'");
        assert_eq!(
            first.label, "Show Trail",
            "first menu item label must be 'Show Trail'"
        );
    }

    /// The main tray menu MUST include "Quit Trail" as the last
    /// item — the user needs a way to exit the menu-bar app
    /// (there's no other visible "Quit" affordance in a menu-bar
    /// app on macOS).
    #[test]
    fn main_tray_menu_ends_with_quit_trail() {
        assert!(
            MAIN_TRAY_MENU_ITEMS.len() >= 2,
            "main tray menu must have at least 2 items (show + quit), got {}",
            MAIN_TRAY_MENU_ITEMS.len()
        );
        let last = MAIN_TRAY_MENU_ITEMS[MAIN_TRAY_MENU_ITEMS.len() - 1];
        assert_eq!(last.id, "quit", "last menu item id must be 'quit'");
        assert_eq!(
            last.label, "Quit Trail",
            "last menu item label must be 'Quit Trail'"
        );
    }

    /// Menu item ids must be unique — the `on_menu_event` closure
    /// matches on the id and a duplicate would silently swallow
    /// the first match (the closure's `_ => {}` arm would never
    /// fire).
    #[test]
    fn main_tray_menu_ids_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for item in MAIN_TRAY_MENU_ITEMS {
            assert!(
                seen.insert(item.id),
                "duplicate tray menu id: {:?}",
                item.id
            );
        }
    }
}
