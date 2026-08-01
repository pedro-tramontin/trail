//! macOS microphone permission detection (Phase 5 §5.7).
//!
//! Provides [`MicPermissionState`] + three pure functions:
//!
//! - [`check_mic_permission`] — read-only status query (no TCC dialog).
//! - [`request_mic_permission`] — prompt the system dialog (on first
//!   call on a fresh install); returns the resulting state.
//! - [`mic_permission_deep_link_url`] — the `x-apple.systempreferences`
//!   URL the tray menu uses for "Open Mic Settings" when permission
//!   is denied.
//!
//! All macOS-only `objc2` calls live behind `#[cfg(target_os =
//! "macos")]`. On Linux / Windows the module compiles to a stub that
//! returns `MicPermissionState::Undetermined` + an empty URL — keeping
//! the rest of the crate (tray, lib) compiling on every host. Real
//! permission verification is a Pedro-on-macOS action per §W4.
//!
//! ## Why `objc2` not `objc` / `cocoa`
//!
//! `objc2` is the modern safe binding maintained by the objc working
//! group; `objc` (block2's older sibling) is now in maintenance-only
//! mode. `objc2 0.6` + `objc2-foundation 0.3` + `block2 0.6` is the
//! 2026-current macOS bindings stack. AVFoundation itself isn't a
//! "binding" crate — we link it via `cargo:rustc-link-lib=framework=
//! AVFoundation` in `build.rs` and reach the C symbols directly
//! through `class!` + `msg_send!`. That's the same approach every
//! Rust+objc2 AVFoundation integration uses.

use std::fmt;

/// The high-level microphone permission state the app cares about.
///
/// Variants map 1:1 onto `AVAuthorizationStatus` + `TCCServiceStatus`
/// — see `check_mic_permission` for the exact integer mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MicPermissionState {
    /// `AVAuthorizationStatusAuthorized` (3). The app may capture
    /// audio without further prompts.
    Granted,
    /// `AVAuthorizationStatusDenied` (1) **or**
    /// `AVAuthorizationStatusRestricted` (2). Both result in the
    /// tray menu surfacing the "Open Mic Settings" item so the user
    /// can fix the TCC state via `x-apple.systempreferences:`.
    Denied,
    /// `AVAuthorizationStatusNotDetermined` (0). The app has never
    /// prompted; the next `request_mic_permission` call will trigger
    /// the TCC dialog.
    Undetermined,
}

impl fmt::Display for MicPermissionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MicPermissionState::Granted => f.write_str("granted"),
            MicPermissionState::Denied => f.write_str("denied"),
            MicPermissionState::Undetermined => f.write_str("undetermined"),
        }
    }
}

/// Read-only check of the current TCC microphone permission state.
///
/// On macOS this calls `+[AVCaptureDevice
/// authorizationStatusForMediaType:AVMediaTypeAudio]` and maps the
/// integer return value to a [`MicPermissionState`]. The call is
/// cheap (no TCC dialog, no audio-thread interaction) and safe to
/// invoke on every tray-menu rebuild.
///
/// On non-macOS hosts the function returns
/// `MicPermissionState::Undetermined` — Linux/Windows would use
/// PipeWire permission / Settings → Privacy → Microphone flows
/// respectively, which are out of scope for v1.
pub fn check_mic_permission() -> MicPermissionState {
    #[cfg(target_os = "macos")]
    {
        macos::authorization_status()
    }
    #[cfg(not(target_os = "macos"))]
    {
        MicPermissionState::Undetermined
    }
}

/// Prompt the user for microphone permission if not yet decided.
///
/// On macOS this calls `+[AVCaptureDevice
/// requestAccessForMediaType:AVMediaTypeAudio completionHandler:]`.
/// The first invocation on a fresh install triggers the TCC dialog;
/// subsequent calls return the previously-recorded answer
/// immediately (macOS caches the TCC decision for the lifetime of
/// the bundle id). The completion handler fires on an arbitrary
/// thread; we block-on it via a `parking_lot::Mutex<Option<bool>>`
/// to keep the public API synchronous — the tray-menu builder
/// calls this once at startup and a synchronous wait is fine.
///
/// On non-macOS this is a no-op returning
/// `MicPermissionState::Undetermined` (the platform-specific
/// permission systems are out of scope for v1).
///
/// **Note:** the v1 tray menu uses [`check_mic_permission`] only
/// (read-only); `request_mic_permission` exists for the future
/// "Test mic" onboarding button that lets Pedro pre-grant the
/// permission before his first live capture.
pub fn request_mic_permission() -> MicPermissionState {
    #[cfg(target_os = "macos")]
    {
        macos::request_access()
    }
    #[cfg(not(target_os = "macos"))]
    {
        MicPermissionState::Undetermined
    }
}

/// Deep-link URL the tray menu hands to `tauri-plugin-opener` when
/// permission is denied. On macOS this is the canonical
/// `x-apple.systempreferences:` URL targeting the Privacy →
/// Microphone pane — the only documented stable way to deep-link
/// into a specific System Settings pane from outside the app
/// itself.
///
/// On non-macOS we return an empty string and the tray-menu filter
/// simply skips the "Open Mic Settings" item (the menu builder
/// already gates it on `MicPermissionState::Denied`).
pub fn mic_permission_deep_link_url() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
    }
    #[cfg(not(target_os = "macos"))]
    {
        ""
    }
}

// =====================================================================
// macOS implementation — wraps the `objc2` calls so the rest of the
// module doesn't need to know about the FFI surface.
// =====================================================================

#[cfg(target_os = "macos")]
mod macos {
    //! `objc2` + `block2` AVFoundation calls. All `unsafe` is
    //! contained here; the public API above is 100% safe.
    //!
    //! `AVCaptureDevice.authorizationStatusForMediaType:` and
    //! `requestAccessForMediaType:completionHandler:` are both
    //! class methods (senders are `metaclass`), returning
    //! `AVAuthorizationStatus` (C `NSInteger`) and `void`
    //! respectively. Status values are:
    //!
    //! - 0 = NotDetermined
    //! - 1 = Restricted
    //! - 2 = Denied
    //! - 3 = Authorized
    //!
    //! (Apple's `AVAuthorizationStatus` enum uses these
    //! historically-fixed integer values; they aren't going to
    //! change without a major SDK rev.)

    use super::MicPermissionState;
    use block2::{Block, RcBlock};
    use objc2::runtime::AnyClass;
    use objc2::{class, msg_send};
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// Map the raw integer status to our enum.
    fn status_from_raw(raw: isize) -> MicPermissionState {
        // Apple-defined AVAuthorizationStatus values. Restricted
        // and Denied collapse into the same `Denied` variant from
        // the tray's perspective — both block capture and both
        // require a manual Settings fix.
        match raw {
            3 => MicPermissionState::Granted,
            1 | 2 => MicPermissionState::Denied,
            _ => MicPermissionState::Undetermined,
        }
    }

    /// Resolve the `AVMediaTypeAudio` extern NSString* exported by
    /// the AVFoundation framework. We import the symbol via
    /// `#[link_name = "AVMediaTypeAudio"]` because objc2 doesn't
    /// ship a typed binding for it (it's a pure-`extern` const,
    /// not a method-bearing class).
    fn av_audio_media_type() -> *const objc2::runtime::AnyObject {
        extern "C" {
            #[link_name = "AVMediaTypeAudio"]
            static AVMediaTypeAudio: objc2::runtime::AnyObject;
        }
        &AVMediaTypeAudio as *const _
    }

    pub fn authorization_status() -> MicPermissionState {
        // SAFETY: `class!` returns a non-null `&'static AnyClass`
        // for any class registered with the ObjC runtime. AVCaptureDevice
        // is registered on every macOS process (it's part of
        // AVFoundation.framework which we link via build.rs). If
        // somehow the class lookup fails (extremely unlikely) we
        // treat it as NotDetermined — same fallback the plan
        // recommends.
        unsafe {
            let cls = class!(AVCaptureDevice);
            if cls.is_null() {
                return MicPermissionState::Undetermined;
            }
            let media_type = av_audio_media_type();
            // Class method — call on the metaclass. `msg_send!`
            // accepts `AnyClass` for class-method calls.
            let raw: isize = msg_send![cls, authorizationStatusForMediaType: media_type];
            status_from_raw(raw)
        }
    }

    pub fn request_access() -> MicPermissionState {
        // SAFETY: same class-pointer invariant as above. The
        // completion handler is a `^void(BOOL granted)` block
        // (Objective-C blocks), bridged via `block2::Block`. We
        // park the calling thread until the system hands us the
        // user's answer — `requestAccessForMediaType:` only takes
        // a few milliseconds to a few seconds (one TCC prompt).
        let granted_slot: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
        let granted_slot_clone = granted_slot.clone();
        let block = RcBlock::new(move |granted: bool| {
            *granted_slot_clone.lock() = Some(granted);
        });
        unsafe {
            let cls = class!(AVCaptureDevice);
            if cls.is_null() {
                return MicPermissionState::Undetermined;
            }
            let media_type = av_audio_media_type();
            // `Block::deref()` recovers the raw `*const Block`
            // pointer for the ObjC call. `requestAccessForMediaType:
            // completionHandler:` is void-returning; the answer is
            // only available inside the block.
            let _: () = msg_send![
                cls,
                requestAccessForMediaType: media_type,
                completionHandler: &*block,
            ];
        }
        // Wait up to 60 seconds for the TCC prompt (the actual
        // user decision typically arrives in <1s; the 60s budget
        // absorbs the case where the dialog is modal over a
        // runaway test). A `None` outcome (no answer) is reported
        // as Undetermined — the next check will surface the
        // real state.
        wait_for_block(granted_slot, std::time::Duration::from_secs(60))
    }

    /// Spin-wait up to `budget` for the block to populate the
    /// granted slot. The block fires on an arbitrary thread; we
    /// don't know which one, so we busy-poll. This is a UI-setup
    /// call (once at startup), not a hot loop — busy-polling
    /// 100ms at most is acceptable.
    fn wait_for_block(
        slot: Arc<Mutex<Option<bool>>>,
        budget: std::time::Duration,
    ) -> MicPermissionState {
        let start = std::time::Instant::now();
        loop {
            if let Some(granted) = *slot.lock() {
                return if granted {
                    MicPermissionState::Granted
                } else {
                    MicPermissionState::Denied
                };
            }
            if start.elapsed() >= budget {
                return MicPermissionState::Undetermined;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn check_mic_permission_returns_a_variant() {
        // The contract is: returns one of the three variants —
        // no panics, no bad returns. On Linux we expect
        // Undetermined; on macOS this exercises the live
        // AVFoundation class lookup.
        let _ = check_mic_permission();
    }

    #[test]
    fn request_mic_permission_does_not_panic() {
        // The v1 tray menu doesn't call this, but the
        // function exists for the future "Test mic" onboarding
        // button. Either outcome (Granted/Denied/Undetermined)
        // is fine — what's tested is that the function exits
        // without panicking on either platform.
        let _ = request_mic_permission();
    }

    #[test]
    fn deep_link_url_format_is_platform_appropriate() {
        // On macOS the URL must point at the Privacy_Microphone
        // pane so the user's one click lands in the right System
        // Settings tab. On Linux/Windows the function returns
        // the empty string (the tray-menu filter already gates
        // the "Open Mic Settings" item on `state == Denied`).
        let url = mic_permission_deep_link_url();
        if cfg!(target_os = "macos") {
            assert!(
                url.contains("Privacy_Microphone"),
                "macOS URL must deep-link the Privacy pane, got {url:?}"
            );
            assert!(
                url.starts_with("x-apple.systempreferences:"),
                "macOS URL must use the system-preferences scheme, got {url:?}"
            );
        } else {
            assert!(
                url.is_empty(),
                "non-macOS URL must be empty (no equivalent deep-link), got {url:?}"
            );
        }
    }
}
