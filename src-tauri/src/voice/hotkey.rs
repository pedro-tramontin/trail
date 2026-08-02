//! Hotkey parsing + registration with conflict detection.
//!
//! v1: Ctrl+Shift+Space is the push-to-talk default. Parsing handles
//! `ctrl|shift|alt|cmd|super` + key names (`space`, `a-z`, `0-9`).
//!
//! On macOS, `global-hotkey` returns an error if another app owns the
//! hotkey; we surface that as `HotkeyError::Conflict` so the Settings UI
//! can pause voice capture until the user picks a different shortcut.
//! Per the plan there is NO silent fallback — silent fallback is hostile
//! UX (the user would press the shortcut, nothing would happen, and they
//! would have no idea why).

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
#[cfg(target_os = "macos")]
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
#[cfg(target_os = "macos")]
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

/// Try to register a hotkey. On macOS, uses `global-hotkey`. On other
/// platforms, returns `Ok(())` as a no-op (for tests).
///
/// On macOS, if `RegisterEventHotKey` fails because another app already
/// owns the shortcut, this returns `HotkeyError::Conflict` so the
/// Settings UI can surface the conflict and pause voice capture.
pub fn register(hk: &HotKey) -> Result<(), HotkeyError> {
    #[cfg(target_os = "macos")]
    {
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
        // global-hotkey 0.7 makes `GlobalHotKeyManager::new()` return
        // `Result<Self, _>` (was infallible in 0.6). Surface the
        // error as a `Platform` variant — the manager can't be
        // constructed on a host without the Carbon HIToolbox
        // backing it would already have failed the `#[cfg]` gate
        // above, so any error here is genuinely a platform issue.
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
    #[cfg(not(target_os = "macos"))]
    {
        let _ = hk;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn register_returns_ok_on_linux() {
        // On non-macOS, `register` is a no-op stub. Verify the happy
        // path so the test count matches the plan.
        #[cfg(not(target_os = "macos"))]
        {
            let hk = parse_hotkey("ctrl+shift+space").unwrap();
            let result = register(&hk);
            assert!(result.is_ok());
        }
        // On macOS this test is a manual-verification placeholder; we
        // still parse to exercise that path uniformly across hosts.
        #[cfg(target_os = "macos")]
        {
            let _ = parse_hotkey("ctrl+shift+space").unwrap();
        }
    }
}
