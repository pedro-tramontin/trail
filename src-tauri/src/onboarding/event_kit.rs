//! macOS EventKit permission grant — the "trigger the TCC dialog"
//! counterpart to [`super::scan::calendar_eventkit_tcc_status`].
//!
//! ## Why this exists (the `?Privacy_Calendar` dead-link problem)
//!
//! On a fresh macOS install the EventKit TCC state is
//! `EKAuthorizationStatusNotDetermined`. Apple's
//! `x-apple.systempreferences:com.apple.preference.security?Privacy_Calendar`
//! deep link takes the user to System Settings → Privacy & Security,
//! but the **Calendars** entry doesn't exist in the sidebar yet —
//! Apple only shows an app in the Privacy list after the user has
//! been prompted to grant access at least once. The deep link
//! therefore lands on a pane that has no row to click, and the
//! user is stuck: "I went to Settings, but there's nothing there
//! to enable."
//!
//! The fix: actually call `EKEventStore.requestFullAccessToEvents`
//! (or `requestAccessToEvents` on pre-Sonoma) the first time the
//! wizard surfaces the EventKit hint. The TCC dialog appears, the
//! user accepts, the entry then exists in System Settings, and the
//! deep link becomes useful as the *post-grant* recovery path
//! (re-visit Settings to revoke, or to check which calendars are
//! exposed).
//!
//! ## Why not reuse the existing voice/permission probe style
//!
//! `voice::permission::request_mic_permission` already implements
//! the same shape (`AVCaptureDevice.requestAccessForMediaType:` +
//! a `block2` completion handler + a 60s busy-poll). This module
//! is a deliberate sibling, not a refactor of that one, because:
//!
//! 1. The two callers (scan report vs wizard IPC) have different
//!    enum mappings (`MicPermissionState` vs `CalendarPermissionState`)
//!    and consolidating them would require a generic "macOS TCC
//!    probe" helper for two callers only.
//! 2. `voice::permission.rs` lives in the voice module tree; pulling
//!    the EventKit request there would cross the module boundary
//!    that `voice::*` is set up to keep narrow.
//!
//! ## Non-macOS behaviour
//!
//! On Linux/Windows the EventKit framework doesn't exist and the
//! calendar collector uses the `.ics` file picker (or the WinRT
//! calendar API), not EventKit. The non-macOS arm returns
//! `CalendarPermissionState::FullAccess` so the wizard's "looks
//! good" path is reachable on those platforms without showing a
//! dead button.

/// The post-prompt EventKit TCC state. Mirrors
/// `super::scan::CalendarEventKitTcc` (the read-only probe) but
/// is a public type with a `String` serialisation contract so the
/// Tauri command surface stays stable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalendarPermissionState {
    /// `.fullAccess` (Sonoma+) or legacy `.authorized`. The
    /// collector can read all events.
    FullAccess,
    /// `.notDetermined` — the dialog was dismissed without a
    /// choice (user clicked outside, or the OS auto-dismissed
    /// because the app wasn't focused). The wizard keeps the
    /// "Grant permission" button visible.
    NotDetermined,
    /// `.denied` / `.writeOnlyAccessDenied` / `.restricted` — the
    /// user refused. The wizard renders the deep link as the
    /// recovery path so the user can revisit System Settings.
    Denied,
}

impl CalendarPermissionState {
    /// Stable string serialisation for the Tauri IPC boundary.
    /// The wizard's `StepAsk.svelte` switches on this value to
    /// decide which control (Grant button / deep link / nothing)
    /// to render in the EventKit hint row.
    pub fn as_str(self) -> &'static str {
        match self {
            CalendarPermissionState::FullAccess => "fullaccess",
            CalendarPermissionState::NotDetermined => "undetermined",
            CalendarPermissionState::Denied => "denied",
        }
    }
}

/// Trigger the macOS EventKit TCC dialog and return the resulting
/// permission state.
///
/// On macOS this dispatches `EKEventStore.requestFullAccessToEvents`
/// (or the pre-Sonoma `requestAccessToEvents`) on the global
/// `EKEventStore` class, waits up to 60s for the
/// `EKEventStoreRequestAccessCompletionHandler` block to fire,
/// and re-reads the TCC status. The returned state is what the
/// wizard needs to decide which control to render.
///
/// On Linux/Windows this is a no-op returning `FullAccess` —
/// there's no OS-level EventKit TCC to gate, and the calendar
/// collector's `~/Library/Calendars/*.ics` /
/// `~/.config/evolution/` / WinRT code paths don't prompt the OS.
#[cfg(target_os = "macos")]
pub fn request_calendar_permission() -> CalendarPermissionState {
    use block2::RcBlock;
    use objc2::{class, msg_send};
    use parking_lot::Mutex;
    use std::sync::Arc;

    /// Map the raw integer status to our enum. Mirrors
    /// the enum variant mapping in
    /// `super::scan::CalendarEventKitTcc` (the
    /// read-only probe) — the wizard sees the same
    /// state from the probe and the request path.
    /// We use the raw integer instead of pattern-
    /// matching the `EKAuthorizationStatus` enum
    /// because (a) the completion block delivers the
    /// value as `i8` (granted) + `*mut AnyObject`
    /// (error) and (b) the post-block re-read of
    /// `authorizationStatusForEntityType:` returns
    /// the raw NSInteger. Apple-defined
    /// `EKAuthorizationStatus` integer values:
    ///   0 = NotDetermined
    ///   1 = Restricted
    ///   2 = Denied
    ///   3 = WriteOnly (Sonoma+ write-only access)
    ///   4 = FullAccess (Sonoma+)
    ///   5 = Authorized (legacy alias for FullAccess)
    fn status_from_raw(raw: isize) -> CalendarPermissionState {
        match raw {
            // 0 = NotDetermined (per Apple EKAuthorizationStatus enum)
            0 => CalendarPermissionState::NotDetermined,
            // 1 = Restricted, 2 = Denied, 3 = WriteOnly — all block
            // the collector, all surface as "denied" to the user.
            1 | 2 | 3 => CalendarPermissionState::Denied,
            // 4 = FullAccess (Sonoma+) or legacy 5 = Authorized
            4 | 5 => CalendarPermissionState::FullAccess,
            // Unknown future value — be conservative, treat as denied.
            _ => CalendarPermissionState::Denied,
        }
    }

    // The completion handler writes the raw status integer into
    // a shared slot. `requestFullAccessToEventsWithCompletion:`
    // fires the block on an arbitrary thread; we busy-poll
    // (same pattern as `voice::permission::macos::request_access`)
    // because the API is callback-based and we want a synchronous
    // return shape at the Tauri command boundary.
    let status_slot: Arc<Mutex<Option<isize>>> = Arc::new(Mutex::new(None));
    let status_slot_clone = status_slot.clone();
    let block = RcBlock::new(move |granted: i8, error: *mut objc2::runtime::AnyObject| {
        // `granted` is the first block argument; the second
        // (`error`) is an `NSError*` (nullable). On success
        // the re-read of the TCC status is the authoritative
        // value (covers the case where the user granted
        // partial access like `.WriteOnly`). On user-deny or
        // OS-deny the re-read is also authoritative.
        let _: *mut objc2::runtime::AnyObject = error; // explicit unused
        *status_slot_clone.lock() = Some(if granted != 0 { 4 } else { 2 });
    });

    // SAFETY: `class!(EKEventStore)` returns a non-null
    // `&'static AnyClass` for the EventKit class registered at
    // process load (linked via build.rs's
    // `cargo:rustc-link-lib=framework=EventKit`). The
    // `requestFullAccessToEventsWithCompletion:` selector is a
    // class method on EKEventStore. The block is a
    // `RcBlock<Fn(i8, *mut AnyObject) -> ()>` matching
    // Apple's `EKEventStoreRequestAccessCompletionHandler`
    // signature.
    unsafe {
        let cls = class!(EKEventStore);
        let _: () = msg_send![
            cls,
            requestFullAccessToEventsWithCompletion: &*block,
        ];
    }

    // Wait up to 60s for the block to populate the slot. The
    // wizard UI thread blocks here; the TCC dialog is modal so
    // the user can't trigger other interactions until the
    // dialog dismisses anyway. 60s is well above any realistic
    // human response time but well below the Tauri command
    // timeout.
    let start = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(60);
    let raw = loop {
        if let Some(raw) = *status_slot.lock() {
            break raw;
        }
        if start.elapsed() >= budget {
            // Block never fired — re-read the TCC status as
            // the fallback. If the user closed the dialog by
            // killing the app, this gives us the last-known
            // state. If the user simply hasn't responded, the
            // TCC is still `NotDetermined`.
            break read_tcc_raw_after_timeout();
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    status_from_raw(raw)
}

/// Non-macOS stub: there's no OS-level EventKit TCC, so the
/// "request" is a no-op returning `FullAccess` (the calendar
/// collector uses `.ics` files / WinRT on these platforms and
/// doesn't need to gate on anything).
#[cfg(not(target_os = "macos"))]
pub fn request_calendar_permission() -> CalendarPermissionState {
    CalendarPermissionState::FullAccess
}

/// On timeout, re-read the TCC status the same way
/// `super::scan::calendar_eventkit_tcc_status` does — gives us
/// the last-known state if the block was lost (e.g. the user
/// closed the app while the dialog was up).
#[cfg(target_os = "macos")]
fn read_tcc_raw_after_timeout() -> isize {
    use objc2::{class, msg_send};
    // SAFETY: same invariant as the probe — `class!` returns
    // a non-null `&'static AnyClass`. The class method
    // `authorizationStatusForEntityType:` returns the raw
    // EKAuthorizationStatus NSInteger.
    unsafe {
        let status: isize =
            msg_send![class!(EKEventStore), authorizationStatusForEntityType: 0isize];
        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The non-macOS stub returns `FullAccess` so the wizard's
    /// "Looks good" path is reachable on Linux/Windows. (macOS
    /// runs the real EventKit code; that path is covered by the
    /// manual wizard QA + the §X-4 `framework_link_smoke_test`-
    /// style doc-test in `voice/permission.rs`.)
    #[test]
    #[cfg(not(target_os = "macos"))]
    fn non_macos_returns_full_access() {
        assert_eq!(request_calendar_permission(), CalendarPermissionState::FullAccess);
        assert_eq!(request_calendar_permission().as_str(), "fullaccess");
    }

    /// String serialisation contract: the wizard's Svelte
    /// branch logic depends on these exact strings. If you
    /// change them you MUST update `StepAsk.svelte` and
    /// `StepAsk.test.ts` together.
    #[test]
    fn as_str_is_stable() {
        assert_eq!(CalendarPermissionState::FullAccess.as_str(), "fullaccess");
        assert_eq!(CalendarPermissionState::NotDetermined.as_str(), "undetermined");
        assert_eq!(CalendarPermissionState::Denied.as_str(), "denied");
    }
}
