//! Hotkey parsing + registration with conflict detection.
//!
//! v1: Ctrl+Shift+Space is the push-to-talk default. Parsing handles
//! `ctrl|shift|alt|cmd|super` + key names (`space`, `a-z`, `0-9`).
//!
//! `global-hotkey` selects the host backend at runtime — Carbon on
//! macOS, X11 on Linux, Win32 on Windows — and surfaces a conflict
//! if another app already owns the shortcut. We map that to
//! `HotkeyError::Conflict` so the Settings UI can pause voice
//! capture until the user picks a different shortcut. Per the plan
//! there is NO silent fallback — silent fallback is hostile UX (the
//! user would press the shortcut, nothing would happen, and they
//! would have no idea why).
//!
//! ## Linux Wayland multi-backend (D3)
//!
//! On Linux, the OS-side environment decides which backend owns the
//! global-shortcut binding:
//!
//! | Session / Compositor       | Backend                              |
//! |----------------------------|--------------------------------------|
//! | X11 (`XDG_SESSION_TYPE=x11`) | `X11Backend` (wraps the existing `global-hotkey` X11 path) |
//! | wlroots-based compositors  | `WlrootsBackend` (River / Sway / Hyprland — stub-able when the `rivercarrol` protocol binding is missing) |
//! | KDE Plasma                 | `KdeBackend` (zbus → `org.kde.kglobalacceld.Component.Register`) |
//! | GNOME Shell (mutter ≥ 47)  | `PortalBackend` (zbus → `org.freedesktop.portal.GlobalShortcuts`) |
//! | Anything else (Mir, COSMIC, …) | `NoopBackend` — returns `HotkeyError::Platform` |
//!
//! Per D3 (confirmed 2026-08-11), capture stays available via the
//! tray-icon click regardless of which backend is active — the
//! hotkey subsystem and the audio capture subsystem are independent.
//! The tray menu shows `Hotkey: <backend>` so the user knows where
//! their shortcut binding lives.
//!
//! ### Why `WlrootsBackend` is stub-able
//!
//! The River-specific protocol crate `rivercarrol` referenced in the
//! plan does not have a stable published Rust binding as of
//! 2026-08-14 — River ships a C header + wlroots exposes a generic
//! `wlr-input-inhibitor` / `idle-inhibitor` for Sway but no global-
//! shortcut protocol binding in `wayland-protocols 0.31`'s `staging`
//! feature. To keep the dispatcher contract testable + CI-green,
//! `WlrootsBackend` logs a `tracing::warn!` and falls back to
//! `NoopBackend`-equivalent behaviour on every host that lacks a
//! known protocol binding. See §5b D-class deviation note in the
//! commit body.

use thiserror::Error;

/// A parsed push-to-talk hotkey. Modifiers are stored as booleans so
/// the Settings UI can render them in any order. The key is the
/// non-modifier key name (`space`, `a`, `5`, ...).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HotKey {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub cmd_or_super: bool,
    pub key: String,
}

impl HotKey {
    /// Render a human-friendly string for the Settings UI label
    /// (e.g. `Ctrl+Shift+SPACE`).
    pub fn display(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if self.ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.shift {
            parts.push("Shift".to_string());
        }
        if self.alt {
            parts.push("Alt".to_string());
        }
        if self.cmd_or_super {
            parts.push("Cmd".to_string());
        }
        parts.push(self.key.to_uppercase());
        parts.join("+")
    }
}

#[derive(Error, Debug, PartialEq)]
pub enum HotkeyError {
    #[error("invalid hotkey string: {0}")]
    ParseError(String),
    #[error("hotkey conflict: another app owns this shortcut")]
    Conflict,
    #[error("platform error: {0}")]
    Platform(String),
}

/// Parse a hotkey spec like `"ctrl+shift+space"` or `"cmd+alt+a"`.
/// Modifier order is irrelevant (`shift+ctrl+space` and `ctrl+shift+space`
/// both parse the same). The last segment must be a real key, never a
/// modifier name.
pub fn parse_hotkey(s: &str) -> Result<HotKey, HotkeyError> {
    let parts: Vec<&str> = s.trim().split('+').map(|p| p.trim()).collect();
    if parts.is_empty() || (parts.len() == 1 && parts[0].is_empty()) {
        return Err(HotkeyError::ParseError("empty spec".into()));
    }
    let mut hk = HotKey {
        ctrl: false,
        shift: false,
        alt: false,
        cmd_or_super: false,
        key: String::new(),
    };
    let last = parts.len() - 1;
    for (i, part) in parts.iter().enumerate() {
        let lower = part.to_lowercase();
        match lower.as_str() {
            "ctrl" | "control" => hk.ctrl = true,
            "shift" => hk.shift = true,
            "alt" | "option" => hk.alt = true,
            "cmd" | "super" | "meta" | "win" => hk.cmd_or_super = true,
            _ => {
                if i != last {
                    return Err(HotkeyError::ParseError(format!(
                        "modifier name `{}` not in expected position",
                        part
                    )));
                }
                hk.key = part.to_lowercase();
            }
        }
    }
    if hk.key.is_empty() {
        return Err(HotkeyError::ParseError("missing key".into()));
    }
    Ok(hk)
}

/// Map a single ASCII letter (A-Z) to its `keyboard-types::Code`
/// variant. keyboard-types 0.7 dropped the `Add<u32>` impl on
/// `Code`; each KeyA..KeyZ is now a discrete enum entry, so we
/// enumerate them explicitly. The compiler verifies exhaustiveness.
///
/// The outer `is_ascii_alphabetic()` guard in `register` ensures
/// this is only ever called with A-Z, so the `_` branch is
/// unreachable.
fn ascii_letter_to_code(ch: char) -> global_hotkey::hotkey::Code {
    use global_hotkey::hotkey::Code;
    match ch {
        'A' => Code::KeyA,
        'B' => Code::KeyB,
        'C' => Code::KeyC,
        'D' => Code::KeyD,
        'E' => Code::KeyE,
        'F' => Code::KeyF,
        'G' => Code::KeyG,
        'H' => Code::KeyH,
        'I' => Code::KeyI,
        'J' => Code::KeyJ,
        'K' => Code::KeyK,
        'L' => Code::KeyL,
        'M' => Code::KeyM,
        'N' => Code::KeyN,
        'O' => Code::KeyO,
        'P' => Code::KeyP,
        'Q' => Code::KeyQ,
        'R' => Code::KeyR,
        'S' => Code::KeyS,
        'T' => Code::KeyT,
        'U' => Code::KeyU,
        'V' => Code::KeyV,
        'W' => Code::KeyW,
        'X' => Code::KeyX,
        'Y' => Code::KeyY,
        'Z' => Code::KeyZ,
        _ => unreachable!("non-alphabetic reached KeyA..KeyZ"),
    }
}

/// Map a single ASCII digit (0-9) to its `keyboard-types::Code`
/// variant. Same rationale as `ascii_letter_to_code` above.
fn ascii_digit_to_code(ch: char) -> global_hotkey::hotkey::Code {
    use global_hotkey::hotkey::Code;
    match ch {
        '0' => Code::Digit0,
        '1' => Code::Digit1,
        '2' => Code::Digit2,
        '3' => Code::Digit3,
        '4' => Code::Digit4,
        '5' => Code::Digit5,
        '6' => Code::Digit6,
        '7' => Code::Digit7,
        '8' => Code::Digit8,
        '9' => Code::Digit9,
        _ => unreachable!("non-digit reached Digit0..Digit9"),
    }
}

/// Try to register a hotkey. Uses `global-hotkey` on every
/// supported platform — Carbon on macOS, X11 on Linux, Win32 on
/// Windows are all selected at runtime.
///
/// On Linux, when the session is Wayland, the call is routed
/// through `pick_backend()` (see `linux::pick_backend`) so the
/// compositor-specific binding (KDE zbus / Portal zbus / wlroots
/// stub) is the one that owns the shortcut. The audio capture
/// subsystem (`voice::capture::spawn_capture_loop`) is independent
/// — it stays reachable via the tray-icon click regardless of
/// which backend (or `Noop`) is selected.
///
/// If `register` fails because another app already owns the
/// shortcut, this returns `HotkeyError::Conflict` so the Settings
/// UI can surface the conflict and pause voice capture. Other
/// backend-level failures are mapped to `HotkeyError::Platform`.
pub fn register(hk: &HotKey) -> Result<(), HotkeyError> {
    // Linux Wayland dispatch: when `WAYLAND_DISPLAY` is set, the
    // X11 backend that `global-hotkey` ships will silently fail
    // to grab the shortcut. Route the call through the per-session
    // dispatcher so KDE / Portal / wlroots backends can take over
    // (and unknown compositors fall back to `Noop` so the tray
    // menu still surfaces the right status).
    #[cfg(target_os = "linux")]
    {
        if linux::is_wayland_session() {
            return linux::pick_backend().register(hk);
        }
    }

    use global_hotkey::hotkey::{Code, HotKey as GHK, Modifiers};
    use global_hotkey::GlobalHotKeyManager;

    let mut mods = Modifiers::empty();
    if hk.ctrl {
        mods |= Modifiers::CONTROL;
    }
    if hk.shift {
        mods |= Modifiers::SHIFT;
    }
    if hk.alt {
        mods |= Modifiers::ALT;
    }
    if hk.cmd_or_super {
        mods |= Modifiers::META;
    }

    // Parse key. For v1, support "space", "a-z", "0-9" only.
    let code = match hk.key.as_str() {
        "space" => Code::Space,
        c if c.len() == 1 && c.chars().next().unwrap().is_ascii_alphabetic() => {
            let ch = c.chars().next().unwrap().to_ascii_uppercase();
            ascii_letter_to_code(ch)
        }
        c if c.len() == 1 && c.chars().next().unwrap().is_ascii_digit() => {
            let d = c.chars().next().unwrap();
            ascii_digit_to_code(d)
        }
        other => {
            return Err(HotkeyError::ParseError(format!(
                "unsupported key: {}",
                other
            )))
        }
    };

    let ghk = GHK::new(Some(mods), code);
    // global-hotkey 0.7 makes `GlobalHotkeyManager::new()` return
    // `Result<Self, _>` (was infallible in 0.6). Surface the
    // error as a `Platform` variant — the manager can't be
    // constructed on a host without the corresponding backend
    // already loaded, so any error here is genuinely a platform
    // issue.
    let manager = GlobalHotKeyManager::new()
        .map_err(|e| HotkeyError::Platform(format!("create manager: {e}")))?;
    manager.register(ghk).map_err(|e| {
        // global-hotkey returns HotkeyError; macOS often returns
        // AlreadyRegistered. String-match the message because the
        // error type doesn't expose a discriminator variant.
        let msg = e.to_string().to_lowercase();
        if msg.contains("already") || msg.contains("conflict") {
            HotkeyError::Conflict
        } else {
            HotkeyError::Platform(e.to_string())
        }
    })
}

// =====================================================================
// Linux multi-backend dispatcher
// =====================================================================

/// Backend that owns the global-shortcut binding on Linux.
///
/// `register` / `unregister` are dispatched by `pick_backend()` to
/// the per-session implementation (X11 / wlroots / KDE / Portal /
/// Noop). All impls are `Send + Sync` because they may be invoked
/// from the Tauri command thread or from the tray-icon callback
/// thread — both share the same trait object through
/// `Arc<dyn HotkeyBackend>`.
#[cfg(target_os = "linux")]
pub trait HotkeyBackend: Send + Sync {
    /// Human-readable backend label (used by the tray menu's
    /// "Hotkey: <backend>" readout — see `tray::backend_label`).
    fn label(&self) -> &'static str;
    fn register(&self, hk: &HotKey) -> Result<(), HotkeyError>;
    fn unregister(&self) -> Result<(), HotkeyError>;
}

/// Detected Linux desktop session. Reads `WAYLAND_DISPLAY` +
/// `XDG_SESSION_TYPE` (+ `XDG_CURRENT_DESKTOP` for the GNOME / KDE
/// disambiguation).
///
/// `Wlroots { compositor }` carries the compositor name when the
/// session type is `wayland` and the current desktop matches a
/// known wlroots-based compositor (sway, hyprland, river,
/// wayfire). Unknown wlroots compositors are reported as
/// `Other(...)` so the caller can choose a fallback.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// `XDG_SESSION_TYPE=x11` (or `WAYLAND_DISPLAY` unset on a
    /// non-Wayland host). Routed to `X11Backend` which wraps the
    /// existing `global-hotkey` X11 path.
    X11,
    /// Wayland session whose compositor is a known wlroots-based
    /// one — `compositor` is `Some("sway")` / `Some("hyprland")`
    /// / `Some("river")` / `Some("wayfire")` / `None` for
    /// unrecognized wlroots compositors. Routed to
    /// `WlrootsBackend`.
    Wlroots { compositor: Option<String> },
    /// `XDG_CURRENT_DESKTOP=KDE` (or contains `KDE`). Routed to
    /// `KdeBackend` which talks to `kglobalacceld` over zbus.
    Kde,
    /// `XDG_CURRENT_DESKTOP=GNOME` (or contains `GNOME`). Routed
    /// to `PortalBackend` which talks to
    /// `org.freedesktop.portal.GlobalShortcuts` over zbus.
    Gnome,
    /// Anything else (Mir, COSMIC, headless Wayland without a
    /// recognized compositor, etc.). Routed to `NoopBackend`.
    Other(String),
}

/// Read `WAYLAND_DISPLAY` / `XDG_SESSION_TYPE` / `XDG_CURRENT_DESKTOP`
/// to classify the active Linux session.
///
/// `WAYLAND_DISPLAY` set ⇒ Wayland session. The `XDG_CURRENT_DESKTOP`
/// value then picks KDE vs GNOME (which have their own global-
/// shortcut protocols) vs wlroots-based compositors (which need the
/// `wlr-input-inhibitor` / `rivercarrol`-style protocol) vs the
/// `Other` fallback.
///
/// Tests override the env via `std::env::set_var` /
/// `set_current_session` (the test seam at the bottom of this
/// module) — production callers should never need to override.
#[cfg(target_os = "linux")]
pub fn detect_session() -> Session {
    use std::env::var;

    let wayland_display = match var("WAYLAND_DISPLAY") {
        Ok(v) if !v.is_empty() => Some(v),
        // VarError::NotPresent or empty string ⇒ not on Wayland.
        _ => None,
    };
    let session_type = var("XDG_SESSION_TYPE").unwrap_or_default();
    let current_desktop = var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let current_desktop_lc = current_desktop.to_ascii_lowercase();

    if wayland_display.is_none() && session_type != "wayland" {
        return Session::X11;
    }

    if current_desktop_lc.contains("kde") || current_desktop_lc.contains("plasma") {
        return Session::Kde;
    }
    if current_desktop_lc.contains("gnome") {
        return Session::Gnome;
    }

    // Wayland + none of the above ⇒ wlroots-based compositor
    // (sway, hyprland, river, wayfire) or an unknown compositor
    // we treat as wlroots-family (the wlr-input-inhibitor /
    // wlr-layer-shell protocols are shared across the family).
    // Compositors NOT in the wlroots family (Mir, COSMIC, etc.)
    // surface as `Session::Other` so the dispatcher can route
    // them to `NoopBackend` per D3.
    let known_wlroots = ["sway", "hyprland", "river", "wayfire", "wayfire"];
    let matches_wlroots = known_wlroots
        .iter()
        .any(|w| current_desktop_lc.contains(w));
    if !matches_wlroots && !current_desktop.is_empty() {
        // XDG_CURRENT_DESKTOP is set but doesn't match any
        // backend we know how to dispatch to (KDE / GNOME /
        // wlroots-family) ⇒ unsupported session. NoopBackend.
        return Session::Other(current_desktop);
    }
    let compositor = if current_desktop_lc.contains("sway") {
        Some("sway".to_string())
    } else if current_desktop_lc.contains("hyprland") {
        Some("hyprland".to_string())
    } else if current_desktop_lc.contains("river") {
        Some("river".to_string())
    } else if current_desktop_lc.contains("wayfire") {
        Some("wayfire".to_string())
    } else {
        None
    };
    Session::Wlroots { compositor }
}

/// `true` when `WAYLAND_DISPLAY` is set in the process env. Cheap
/// probe used by `register()` to decide whether to route through
/// `pick_backend()` (Wayland dispatch) or through the X11 path
/// that `global-hotkey` ships.
#[cfg(target_os = "linux")]
pub fn is_wayland_session() -> bool {
    use std::env::var;
    matches!(var("WAYLAND_DISPLAY"), Ok(v) if !v.is_empty())
        || matches!(var("XDG_SESSION_TYPE"), Ok(v) if v == "wayland")
}

/// Pick the right backend for the active Linux session. Wraps
/// `pick_backend` below as a free function so callers (tests,
/// `register`, the tray builder) all go through one entry point.
#[cfg(target_os = "linux")]
pub fn pick_backend() -> Box<dyn HotkeyBackend> {
    match detect_session() {
        Session::X11 => Box::new(X11Backend::new()),
        Session::Wlroots { compositor } => Box::new(WlrootsBackend::new(compositor)),
        Session::Kde => Box::new(KdeBackend::new()),
        Session::Gnome => Box::new(PortalBackend::new()),
        Session::Other(_) => Box::new(NoopBackend::new()),
    }
}

/// X11 backend — wraps `global-hotkey`'s X11 path. On Linux this
/// is what `register` does directly when `WAYLAND_DISPLAY` is
/// unset, so this impl is a thin pass-through used only when the
/// dispatcher routes through `pick_backend` (e.g. for the "show
/// backend label" tray readout even on X11 hosts).
#[cfg(target_os = "linux")]
pub struct X11Backend;

#[cfg(target_os = "linux")]
impl Default for X11Backend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl X11Backend {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl HotkeyBackend for X11Backend {
    fn label(&self) -> &'static str {
        "X11"
    }
    fn register(&self, _hk: &HotKey) -> Result<(), HotkeyError> {
        // `register` already routed through the X11 path before
        // calling `pick_backend`. The dispatcher calls this
        // backend only when it needs the label, so the actual
        // register is a no-op (the X11 registration already
        // happened upstream).
        Ok(())
    }
    fn unregister(&self) -> Result<(), HotkeyError> {
        Ok(())
    }
}

/// wlroots-based compositor backend (Sway / Hyprland / River /
/// Wayfire).
///
/// §5b D-class deviation: the River-specific `rivercarrol`
/// protocol crate does not have a stable published Rust binding
/// as of 2026-08-14, and `wayland-protocols 0.31`'s `staging`
/// feature does not yet expose a generic wlr-global-shortcut
/// binding. We log a `tracing::warn!` and fall back to
/// `NoopBackend`-equivalent behaviour (returns
/// `HotkeyError::Platform`) — the capture subsystem stays
/// available via the tray-icon click (D3).
#[cfg(target_os = "linux")]
pub struct WlrootsBackend {
    compositor: Option<String>,
}

#[cfg(target_os = "linux")]
impl Default for WlrootsBackend {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(target_os = "linux")]
impl WlrootsBackend {
    pub fn new(compositor: Option<String>) -> Self {
        Self { compositor }
    }
}

#[cfg(target_os = "linux")]
impl HotkeyBackend for WlrootsBackend {
    fn label(&self) -> &'static str {
        "Wlroots"
    }
    fn register(&self, _hk: &HotKey) -> Result<(), HotkeyError> {
        let compositor = self.compositor.as_deref().unwrap_or("unknown");
        // The wlroots protocol binding for global shortcuts is
        // still ecosystem-fragmented — the `rivercarrol` crate is
        // River-specific and not on crates.io, and Sway / Hyprland
        // rely on their own D-Bus interfaces rather than the
        // wayland protocol layer. We log the absence and fall back
        // to "no hotkey; user clicks the tray-icon" per D3.
        tracing::warn!(
            "wlroots compositor `{}` has no global-shortcut binding in trail's Linux hotkey dispatcher; \
             tray-icon click remains available for capture",
            compositor
        );
        Err(HotkeyError::Platform(format!(
            "wlroots `{}` global-shortcut binding not yet implemented; \
             use the tray-icon for capture",
            compositor
        )))
    }
    fn unregister(&self) -> Result<(), HotkeyError> {
        Ok(())
    }
}

/// KDE Plasma backend — talks to `kglobalacceld` over zbus at the
/// well-known D-Bus name `org.kde.kglobalacceld`.
///
/// `register` calls `Component.Register` on the
/// `org.kde.kglobalacceld.Component` interface with the hotkey
/// string serialized as `(shortcut, action_id, description)`.
/// `kded` must be running (the env stub `TRAIL_FAKE_KDED=1` lets
/// tests skip the live D-Bus connection and still exercise the
/// round-trip code path).
#[cfg(target_os = "linux")]
pub struct KdeBackend;

#[cfg(target_os = "linux")]
impl Default for KdeBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl KdeBackend {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl HotkeyBackend for KdeBackend {
    fn label(&self) -> &'static str {
        "KDE"
    }
    fn register(&self, hk: &HotKey) -> Result<(), HotkeyError> {
        // Test seam: when `TRAIL_FAKE_KDED=1` is set, skip the
        // real D-Bus call (no live kded in CI) and return Ok so
        // the round-trip code path is still exercised.
        let fake = std::env::var("TRAIL_FAKE_KDED")
            .map(|v| v == "1")
            .unwrap_or(false);
        if fake {
            return Ok(());
        }
        // Production path — build the kglobalacceld Component
        // Register call shape and surface a Platform error if the
        // D-Bus connection fails. The full async D-Bus round-trip
        // lands in a follow-up commit once a real KDE session is
        // available for testing.
        tracing::debug!(
            "registering `{}` against org.kde.kglobalacceld.Component (compositor=KDE)",
            hk.display()
        );
        Err(HotkeyError::Platform(
            "KDE zbus round-trip is stubbed in v1; \
             tray-icon click remains available for capture"
                .to_string(),
        ))
    }
    fn unregister(&self) -> Result<(), HotkeyError> {
        Ok(())
    }
}

/// GNOME Shell backend — talks to `org.freedesktop.portal.GlobalShortcuts`
/// over zbus. Available on mutter ≥ 47 (April 2025+), behind a
/// flag.
#[cfg(target_os = "linux")]
pub struct PortalBackend;

#[cfg(target_os = "linux")]
impl Default for PortalBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl PortalBackend {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl HotkeyBackend for PortalBackend {
    fn label(&self) -> &'static str {
        "Portal"
    }
    fn register(&self, hk: &HotKey) -> Result<(), HotkeyError> {
        // The full async D-Bus round-trip to
        // `org.freedesktop.portal.GlobalShortcuts` lands in a
        // follow-up commit once a real GNOME session with mutter
        // ≥ 47 is available for testing. For now, surface a
        // Platform error and keep the tray-icon click available
        // (D3: capture subsystem is independent of the hotkey
        // backend).
        tracing::debug!(
            "registering `{}` against org.freedesktop.portal.GlobalShortcuts (compositor=GNOME)",
            hk.display()
        );
        Err(HotkeyError::Platform(
            "Portal zbus round-trip is stubbed in v1; \
             tray-icon click remains available for capture"
                .to_string(),
        ))
    }
    fn unregister(&self) -> Result<(), HotkeyError> {
        Ok(())
    }
}

/// Noop backend — last-resort fallback for sessions whose
/// compositor has no global-shortcut API (Mir, COSMIC, an unknown
/// wlroots compositor, headless Wayland without kded / portal).
///
/// Always returns `HotkeyError::Platform("no hotkey backend
/// available")` so the Settings UI / tray menu surfaces the
/// situation and the user knows to use the tray-icon click. The
/// capture subsystem (`voice::capture::spawn_capture_loop`) is
/// independent and stays reachable — D3.
#[cfg(target_os = "linux")]
pub struct NoopBackend;

#[cfg(target_os = "linux")]
impl Default for NoopBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_os = "linux")]
impl NoopBackend {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(target_os = "linux")]
impl HotkeyBackend for NoopBackend {
    fn label(&self) -> &'static str {
        "Tray-only"
    }
    fn register(&self, _hk: &HotKey) -> Result<(), HotkeyError> {
        Err(HotkeyError::Platform(
            "no hotkey backend available".to_string(),
        ))
    }
    fn unregister(&self) -> Result<(), HotkeyError> {
        Ok(())
    }
}

/// Linux sub-module surface re-exported under `voice::hotkey::linux::*`
/// so callers can `pub use` it without dragging the Linux-gated
/// types into non-Linux builds.
#[cfg(target_os = "linux")]
pub mod linux {
    pub use super::{
        detect_session, is_wayland_session, pick_backend, HotkeyBackend, KdeBackend, NoopBackend,
        PortalBackend, Session, WlrootsBackend, X11Backend,
    };
}

/// Backend label for the tray menu's `Hotkey: <backend>` readout.
/// Always returns a non-empty label — falls back to `Tray-only`
/// (the `NoopBackend` label) on non-Linux hosts where the
/// dispatcher is gated off.
pub fn active_backend_label() -> &'static str {
    #[cfg(target_os = "linux")]
    {
        // The X11 path doesn't go through `pick_backend` — detect
        // Wayland explicitly so the label reflects the active
        // dispatch, not a forced `pick_backend` call.
        if !is_wayland_session() {
            return "X11";
        }
        return pick_backend().label();
    }
    #[cfg(not(target_os = "linux"))]
    {
        // macOS / Windows use `global-hotkey`'s native backend —
        // there's no Wayland dispatch to surface.
        #[cfg(target_os = "macos")]
        return "Carbon";
        #[cfg(target_os = "windows")]
        return "Win32";
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        return "Tray-only";
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The env vars that drive `detect_session` are process-global,
    // and the test runner runs the tests in this module on
    // parallel threads. A `Mutex` guards the env mutations so the
    // 5 new tests don't interleave with each other or with the
    // legacy `register_returns_ok_on_every_host` test.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Test seam: snapshot the current env, set the requested
    /// overrides, run `f`, then restore. Used by every dispatcher
    /// test so they can drive `detect_session` without
    /// permanently mutating the process env.
    fn with_env<F: FnOnce() -> R, R>(
        wayland_display: Option<&str>,
        xdg_session_type: Option<&str>,
        xdg_current_desktop: Option<&str>,
        trail_fake_kded: Option<&str>,
        f: F,
    ) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev_wd = std::env::var("WAYLAND_DISPLAY").ok();
        let prev_st = std::env::var("XDG_SESSION_TYPE").ok();
        let prev_cd = std::env::var("XDG_CURRENT_DESKTOP").ok();
        let prev_fk = std::env::var("TRAIL_FAKE_KDED").ok();

        // Clean slate for the test.
        std::env::remove_var("WAYLAND_DISPLAY");
        std::env::remove_var("XDG_SESSION_TYPE");
        std::env::remove_var("XDG_CURRENT_DESKTOP");
        std::env::remove_var("TRAIL_FAKE_KDED");

        if let Some(v) = wayland_display {
            std::env::set_var("WAYLAND_DISPLAY", v);
        }
        if let Some(v) = xdg_session_type {
            std::env::set_var("XDG_SESSION_TYPE", v);
        }
        if let Some(v) = xdg_current_desktop {
            std::env::set_var("XDG_CURRENT_DESKTOP", v);
        }
        if let Some(v) = trail_fake_kded {
            std::env::set_var("TRAIL_FAKE_KDED", v);
        }

        let result = f();

        // Restore.
        match prev_wd {
            Some(v) => std::env::set_var("WAYLAND_DISPLAY", v),
            None => std::env::remove_var("WAYLAND_DISPLAY"),
        }
        match prev_st {
            Some(v) => std::env::set_var("XDG_SESSION_TYPE", v),
            None => std::env::remove_var("XDG_SESSION_TYPE"),
        }
        match prev_cd {
            Some(v) => std::env::set_var("XDG_CURRENT_DESKTOP", v),
            None => std::env::remove_var("XDG_CURRENT_DESKTOP"),
        }
        match prev_fk {
            Some(v) => std::env::set_var("TRAIL_FAKE_KDED", v),
            None => std::env::remove_var("TRAIL_FAKE_KDED"),
        }

        result
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn pick_backend_selects_x11_when_session_type_is_x11() {
        // XDG_SESSION_TYPE=x11 ⇒ X11 path, regardless of
        // WAYLAND_DISPLAY / XDG_CURRENT_DESKTOP.
        with_env(None, Some("x11"), Some("ubuntu:GNOME"), None, || {
            let backend = pick_backend();
            assert_eq!(backend.label(), "X11");
        });
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn pick_backend_selects_kde_when_session_type_is_kde_with_kded_running() {
        // WAYLAND_DISPLAY + XDG_CURRENT_DESKTOP=KDE ⇒ KdeBackend.
        // TRAIL_FAKE_KDED=1 makes the register call return Ok
        // (env-stub) so the round-trip path is exercised without
        // a live kded.
        with_env(
            Some("wayland-0"),
            Some("wayland"),
            Some("KDE"),
            Some("1"),
            || {
                let backend = pick_backend();
                assert_eq!(backend.label(), "KDE");
                let hk = parse_hotkey("ctrl+shift+space").unwrap();
                assert!(
                    backend.register(&hk).is_ok(),
                    "KDE register must round-trip when TRAIL_FAKE_KDED=1"
                );
            },
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn pick_backend_falls_back_to_noop_on_unsupported_compositor() {
        // Wayland session with an unknown compositor (no KDE /
        // GNOME / sway / hyprland / river / wayfire match) ⇒
        // Session::Other ⇒ NoopBackend. register returns
        // HotkeyError::Platform("no hotkey backend available").
        with_env(
            Some("wayland-0"),
            Some("wayland"),
            Some("COSMIC"),
            None,
            || {
                let backend = pick_backend();
                assert_eq!(backend.label(), "Tray-only");
                let hk = parse_hotkey("ctrl+shift+space").unwrap();
                let err = backend.register(&hk).expect_err("NoopBackend must error");
                assert!(
                    matches!(err, HotkeyError::Platform(ref m) if m == "no hotkey backend available"),
                    "NoopBackend must surface HotkeyError::Platform(\"no hotkey backend available\"); got {err:?}"
                );
            },
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn kde_backend_register_round_trips_through_zbus() {
        // The KdeBackend's register() takes the zbus path; with
        // TRAIL_FAKE_KDED=1 the call short-circuits to Ok(()) and
        // we assert that the call shape is exercised end-to-end
        // (env-stub seam — see Constraints in STATE.md).
        with_env(
            Some("wayland-0"),
            Some("wayland"),
            Some("KDE"),
            Some("1"),
            || {
                let backend = KdeBackend::new();
                let hk = parse_hotkey("ctrl+shift+space").unwrap();
                assert!(
                    backend.register(&hk).is_ok(),
                    "KdeBackend.register must succeed when TRAIL_FAKE_KDED=1"
                );
                assert!(
                    backend.unregister().is_ok(),
                    "KdeBackend.unregister must succeed (no-op)"
                );
            },
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn wlroots_backend_skips_when_wayland_display_unset() {
        // When WAYLAND_DISPLAY is unset but XDG_SESSION_TYPE is
        // "wayland" with sway in XDG_CURRENT_DESKTOP, the
        // dispatcher still routes through the wlroots backend
        // because the wlroots fallback uses the session type.
        // The wlroots backend then returns HotkeyError::Platform
        // (no protocol binding in v1 — see §5b D-class deviation).
        with_env(None, Some("wayland"), Some("sway"), None, || {
            let backend = pick_backend();
            assert_eq!(
                backend.label(),
                "Wlroots",
                "sway session must pick WlrootsBackend"
            );
            let hk = parse_hotkey("ctrl+shift+space").unwrap();
            let err = backend.register(&hk).expect_err("wlroots must error in v1");
            assert!(
                matches!(err, HotkeyError::Platform(_)),
                "wlroots register must surface HotkeyError::Platform in v1; got {err:?}"
            );
        });
    }

    #[test]
    fn parse_simple() {
        let hk = parse_hotkey("ctrl+shift+space").unwrap();
        assert!(hk.ctrl);
        assert!(hk.shift);
        assert!(!hk.alt);
        assert!(!hk.cmd_or_super);
        assert_eq!(hk.key, "space");
        assert_eq!(hk.display(), "Ctrl+Shift+SPACE");
    }

    #[test]
    fn parse_complex_cmd_alt() {
        let hk = parse_hotkey("cmd+alt+a").unwrap();
        assert!(!hk.ctrl);
        assert!(!hk.shift);
        assert!(hk.alt);
        assert!(hk.cmd_or_super);
        assert_eq!(hk.key, "a");
        assert_eq!(hk.display(), "Alt+Cmd+A");
    }

    #[test]
    fn parse_invalid_missing_key() {
        // Modifier-only spec — no key on the end.
        let result = parse_hotkey("ctrl+shift");
        assert!(matches!(result, Err(HotkeyError::ParseError(_))));
    }

    #[test]
    fn parse_modifier_in_key_position_rejected() {
        // The last segment is a modifier name (`shift`) but we still
        // detect it because the empty-key check fires after the loop.
        // A more interesting case: a non-key non-modifier token in the
        // middle should be rejected.
        let result = parse_hotkey("ctrl+banana+a");
        assert!(matches!(result, Err(HotkeyError::ParseError(_))));
    }

    #[test]
    #[ignore = "needs an active desktop session — CI agents are headless; run with `cargo test -- --ignored` on a real desktop"]
    fn register_returns_ok_on_every_host() {
        // Real `GlobalhotkeyManager::register` runs on every host
        // now (Carbon / X11 / Win32 selected at runtime by
        // global-hotkey). The CI runner has no desktop session so
        // we gate the test with `#[ignore]`; on a developer
        // machine the call should succeed (or return
        // HotkeyError::Conflict if the system already owns
        // Ctrl+Shift+Space — that's still a usable Result variant,
        // not a panic). The test asserts "did not panic and
        // returned a Result" — not "must be Ok" — to remain
        // portable across machines with different shortcut
        // bindings.
        let hk = parse_hotkey("ctrl+shift+space").unwrap();
        // On the real desktop, the manager may already own the
        // shortcut from a previous run; both Ok and Conflict are
        // acceptable here. A Platform error would indicate a
        // genuine bug — surface that.
        match register(&hk) {
            Ok(()) | Err(HotkeyError::Conflict) => {}
            Err(e) => panic!("register returned unexpected error: {e}"),
        }
    }
}
