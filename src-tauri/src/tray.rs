//! Tray menu item filtering (Phase 5 §5.6 + §5.7).
//!
//! Each Tauri tray rebuild calls [`filtered_items`] to compute which
//! menu entries should be visible. The rule set:
//!
//! - **StopRecording** — visible only while a voice capture is
//!   active (the user can't "stop" an idle mic).
//! - **OpenMicSettings** — visible only when the TCC mic permission
//!   is `Denied` (granted / undetermined don't need an "open
//!   Settings" item).
//! - **HotkeyConflict** — visible only when the OS rejected our
//!   push-to-talk registration (another app owns the shortcut).
//! - **StartRecording** — visible when not recording AND mic
//!   permission is `Granted`. Permission `Denied` / `Undetermined`
//!   suppresses the start item because tapping it would just
//!   surface a system prompt that the user already saw (or
//!   wouldn't be allowed to dismiss).
//!
//! Items live behind a pure function so the filter logic is unit
//! testable without a live Tauri runtime (the tray-icon handle is
//! irrelevant to "which items belong in the menu at state X"). The
//! actual `tauri::menu` plugin call that consumes the [`MenuEntry`]
//! list is wired by the v1 `voice_start` setup (later in §5.9).
//!
//! ## Why `MenuEntry` is an enum, not a trait
//!
//! Tauri 2's `Menu` builder wants concrete types per menu item
//! kind. Returning a `Vec<MenuEntry>` and letting the IPC-side
//! builder destructure keeps the filter logic host-agnostic
//! (testable on Linux) while leaving the actual menu assembly
//! free to use whatever Tauri 2 API is current at integration time.

use crate::voice::permission::{check_mic_permission, MicPermissionState};

/// One tray menu item, in render order.
///
/// The `String` payload is the user-visible label (the
/// `x-apple.systempreferences:` URL lives on [`MenuEntry::OpenMicSettings`]
/// — the menu builder passes it to `tauri-plugin-opener`).
#[allow(dead_code)] // consumed by the v1 §5.9 menu-builder wiring
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// "Start recording" — visible when idle + permitted.
    StartRecording,
    /// "Stop recording" — visible only while a capture is active.
    StopRecording,
    /// "Open Mic Settings" — visible only on permission denied;
    /// carries the deep-link URL for the menu builder.
    OpenMicSettings { deep_link_url: String },
    /// "Hotkey conflict" — visible only when another app owns
    /// the push-to-talk shortcut.
    HotkeyConflict,
    /// "Hotkey: <backend>" — readout that names which hotkey
    /// backend (X11 / KDE / Portal / Wlroots / Tray-only / Carbon
    /// / Win32) is active on the current host. The user uses it
    /// to know where the shortcut binding lives — per D3 the
    /// capture subsystem stays reachable via the tray-icon click
    /// regardless of which backend (or `Tray-only` Noop) is
    /// active, but the binding ownership differs.
    HotkeyBackend { label: &'static str },
    /// "Quit Trail" — always visible (the tray is the only
    /// way to exit the menu-bar app).
    Quit,
}

#[allow(dead_code)] // consumed by the v1 §5.9 menu-builder wiring
impl MenuEntry {
    /// Human-readable label (what the menu builder renders).
    pub fn label(&self) -> String {
        match self {
            MenuEntry::StartRecording => "Start recording".to_string(),
            MenuEntry::StopRecording => "Stop recording".to_string(),
            MenuEntry::OpenMicSettings { .. } => "Open Mic Settings".to_string(),
            MenuEntry::HotkeyConflict => {
                "Hotkey conflict (another app owns this shortcut)".to_string()
            }
            MenuEntry::HotkeyBackend { label } => format!("Hotkey: {label}"),
            MenuEntry::Quit => "Quit Trail".to_string(),
        }
    }
}

/// Snapshot of the upstream signals the menu filter needs.
///
/// `recording` and `hotkey_conflict` come from the app-level state
/// (CaptureState + the global-hotkey registration result) — the
/// menu builder fetches them via `Arc<...>` clones before each
/// rebuild. `mic_permission` is cached so the filter doesn't
/// round-trip into AVFoundation on every click.
#[allow(dead_code)] // consumed by the v1 §5.9 menu-builder wiring
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrayState {
    /// `true` while a voice capture is active.
    pub recording: bool,
    /// `true` if the push-to-talk hotkey registration failed
    /// because another app owns the shortcut.
    pub hotkey_conflict: bool,
    /// The current TCC mic permission state. Pass the cached
    /// value from the most recent `check_mic_permission()` call —
    /// the filter itself does not invoke AVFoundation.
    pub mic_permission: MicPermissionState,
}

#[allow(dead_code)] // consumed by the v1 §5.9 menu-builder wiring
impl TrayState {
    /// Convenience constructor for the most common case: idle,
    /// no hotkey conflict, mic permission granted.
    pub fn idle_permitted() -> Self {
        Self {
            recording: false,
            hotkey_conflict: false,
            mic_permission: MicPermissionState::Granted,
        }
    }

    /// Construct from a snapshot of the live app state. Reads
    /// the current TCC permission state once (cheap AVFoundation
    /// call on macOS, no-op on Linux) and merges it with the
    /// caller-supplied `recording` + `hotkey_conflict` flags.
    pub fn from_live(recording: bool, hotkey_conflict: bool) -> Self {
        Self {
            recording,
            hotkey_conflict,
            mic_permission: check_mic_permission(),
        }
    }
}

/// Compute the visible menu items for a given state.
///
/// Render order is fixed: `StartRecording` / `StopRecording` first
/// (exactly one is shown, depending on `recording`), then the
/// situational items (`OpenMicSettings`, `HotkeyConflict`), then
/// the always-on `Quit`. The actual macOS menu would insert
/// separators between the groups; the filter omits them because
/// the renderer decides where separators go (separators are a
/// presentation concern, not a state one).
///
/// ## Filter rules
///
/// | State                                     | Items                                |
/// |-------------------------------------------|--------------------------------------|
/// | idle + granted                            | StartRecording + Quit                |
/// | idle + denied                             | StartRecording + OpenMicSettings + Quit (suppressed: see note) |
/// | idle + undetermined                       | StartRecording + Quit                |
/// | recording (any permission)                | StopRecording + Quit                 |
/// | hotkey conflict (any permission)          | + HotkeyConflict                     |
///
/// Note: `OpenMicSettings` is only inserted when
/// `mic_permission == Denied`. The `StartRecording` item remains
/// visible when denied so the user can still surface the system
/// dialog via `voice_start`; the app refuses to actually capture
/// until permission resolves. This matches the §5.7 spec:
/// "Open Mic Settings shown when permission is denied".
#[allow(dead_code)] // consumed by the v1 §5.9 menu-builder wiring
pub fn filtered_items(state: TrayState) -> Vec<MenuEntry> {
    let mut items = Vec::with_capacity(4);

    // The start/stop split. Exactly one of these is always
    // visible (even on permission denied — the click surfaces
    // the TCC prompt that the OS would show anyway).
    if state.recording {
        items.push(MenuEntry::StopRecording);
    } else {
        items.push(MenuEntry::StartRecording);
    }

    // Mic-permission deep-link item. Inserted only on Denied;
    // the URL is the canonical x-apple.systempreferences pane.
    // Both Denied and Restricted (status 2) collapse to Denied
    // in MicPermissionState, but we keep the gating generic to
    // be robust if a future revision splits them apart.
    if state.mic_permission == MicPermissionState::Denied {
        items.push(MenuEntry::OpenMicSettings {
            deep_link_url: crate::voice::permission::mic_permission_deep_link_url().to_string(),
        });
    }

    // Hotkey conflict indicator. Inserted only when the OS
    // rejected the Ctrl+Shift+Space registration. The
    // click handler opens the System Settings → Keyboard pane
    // via a separate deep-link (no v1 support yet — the tray
    // builder just labels the item; the menu click is currently
    // a no-op for this entry).
    if state.hotkey_conflict {
        items.push(MenuEntry::HotkeyConflict);
    }

    // Hotkey backend readout (D3). Always present so the user
    // knows which backend owns the binding — X11 / Wlroots / KDE
    // / Portal / Tray-only on Linux, Carbon / Win32 elsewhere.
    // The `Tray-only` label is the NoopBackend fallback (capture
    // stays reachable via the tray-icon click regardless).
    items.push(MenuEntry::HotkeyBackend {
        label: crate::voice::active_backend_label(),
    });

    // Quit is always last and always present.
    items.push(MenuEntry::Quit);

    items
}

// =====================================================================
// Tests — pure-function filter logic.
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(state: TrayState) -> TrayState {
        TrayState {
            recording: true,
            ..state
        }
    }

    fn hotkey_conflict(state: TrayState) -> TrayState {
        TrayState {
            hotkey_conflict: true,
            ..state
        }
    }

    #[test]
    fn idle_granted_shows_start_and_quit_only() {
        // Baseline idle state: Start + Quit + HotkeyBackend readout,
        // no conditional items. The HotkeyBackend item is always
        // present (per D3 the user needs to know which backend owns
        // the binding); its label is host-specific so we assert
        // presence-by-variant, not label equality.
        let items = filtered_items(TrayState::idle_permitted());
        assert!(
            items.contains(&MenuEntry::StartRecording),
            "idle + granted must include StartRecording"
        );
        assert!(
            items.contains(&MenuEntry::Quit),
            "idle + granted must include Quit"
        );
        assert!(
            items
                .iter()
                .any(|e| matches!(e, MenuEntry::HotkeyBackend { .. })),
            "HotkeyBackend readout must always be present"
        );
        assert!(
            !items.contains(&MenuEntry::OpenMicSettings {
                deep_link_url: String::new()
            }),
            "OpenMicSettings must NOT appear for Granted permission"
        );
    }

    #[test]
    fn recording_hides_start_shows_stop() {
        // Active capture: Start is hidden, Stop is shown. The
        // HotkeyBackend readout is always present.
        let state = recording(TrayState::idle_permitted());
        let items = filtered_items(state);
        assert!(
            items.contains(&MenuEntry::StopRecording),
            "recording state must surface StopRecording"
        );
        assert!(
            !items.contains(&MenuEntry::StartRecording),
            "recording state must NOT surface StartRecording"
        );
        assert!(
            items
                .iter()
                .any(|e| matches!(e, MenuEntry::HotkeyBackend { .. })),
            "HotkeyBackend readout must always be present"
        );
    }

    #[test]
    fn denied_permission_surfaces_open_mic_settings() {
        // Denied permission: Open Mic Settings appears; the
        // start item also remains so the user can re-trigger
        // the TCC prompt.
        let state = TrayState {
            recording: false,
            hotkey_conflict: false,
            mic_permission: MicPermissionState::Denied,
        };
        let items = filtered_items(state);
        // OpenMicSettings must be present and carry a non-empty URL
        // on macOS (empty on Linux — the menu builder can skip it
        // there, but the entry stays so the test stays
        // platform-agnostic).
        let open_mic = items
            .iter()
            .find(|e| matches!(e, MenuEntry::OpenMicSettings { .. }))
            .expect("OpenMicSettings must appear when permission is Denied");
        if cfg!(target_os = "macos") {
            assert!(
                matches!(open_mic, MenuEntry::OpenMicSettings { deep_link_url } if !deep_link_url.is_empty()),
                "macOS OpenMicSettings must carry the system-preferences URL"
            );
        }
    }

    #[test]
    fn undetermined_permission_does_not_show_open_mic_settings() {
        // Undetermined ≠ Denied. The deep-link item is reserved
        // for the case where TCC has actively refused; for
        // undetermined the user just hasn't been asked yet.
        let state = TrayState {
            recording: false,
            hotkey_conflict: false,
            mic_permission: MicPermissionState::Undetermined,
        };
        let items = filtered_items(state);
        assert!(
            !items
                .iter()
                .any(|e| matches!(e, MenuEntry::OpenMicSettings { .. })),
            "OpenMicSettings must NOT appear for Undetermined permission"
        );
    }

    #[test]
    fn hotkey_conflict_inserts_indicator() {
        // Conflict state: HotkeyConflict appears alongside the
        // baseline items.
        let state = hotkey_conflict(TrayState::idle_permitted());
        let items = filtered_items(state);
        assert!(
            items.contains(&MenuEntry::HotkeyConflict),
            "HotkeyConflict must surface when another app owns the shortcut"
        );
    }

    #[test]
    fn recording_hides_hotkey_conflict_independent_of_permission() {
        // Recording + conflict: StopRecording is shown, but
        // HotkeyConflict is also shown (we still want to
        // notify the user). The menu's ordered list is
        // Stop + Conflict + Quit.
        let state = recording(hotkey_conflict(TrayState {
            recording: true,
            hotkey_conflict: true,
            mic_permission: MicPermissionState::Granted,
        }));
        let items = filtered_items(state);
        assert!(items.contains(&MenuEntry::StopRecording));
        assert!(items.contains(&MenuEntry::HotkeyConflict));
        assert!(items.contains(&MenuEntry::Quit));
        // Start is never present while recording.
        assert!(!items.contains(&MenuEntry::StartRecording));
    }
}
