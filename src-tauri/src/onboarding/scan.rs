//! Non-invasive laptop scan for Phase 6 onboarding.
//!
//! The scan is purely read-only. It probes a curated list of "could
//! be useful" data sources on the user's machine (GitHub CLI config,
//! Claude sessions, Gmail's host Mail.app, Apple Notes, VS Code
//! extensions, Chrome + Brave history files, etc.) and reports which
//! collectors Trail could plausibly install. **It never reads the
//! files themselves** — Privacy: the Chrome/Brave History SQLite is
//! locked while the browser runs, and even when it isn't we never
//! want to be in the business of reading browser history during
//! setup. That read happens later, in item 6-2 (llm-onboarding-qa),
//! which only runs AFTER the user explicitly opts in.
//!
//! Output: a typed [`ScanReport`] keyed by `collector_id` (e.g.
//! `"github"`, `"claude_sessions"`) with one [`CollectorCandidate`]
//! per known source. The frontend's onboarding wizard (item 6-4)
//! consumes this report to render checkboxes for the user to opt
//! into per-collector.
//!
//! ## Status semantics
//!
//! - [`CollectorStatus::Available`] — we found evidence that the
//!   source is present, but no `~/.trail/config.json` exists for it
//!   yet (i.e. the user hasn't enabled it via a prior onboarding).
//! - [`CollectorStatus::Unavailable`] — no evidence found. Could be
//!   that the user genuinely doesn't use that app, or that the path
//!   is just empty. Either way, offering the collector would be
//!   useless.
//! - [`CollectorStatus::AlreadyConfigured`] — the collector id
//!   appears in the loaded `Config::pending_installs` (or otherwise
//!   already has a non-empty config slice), so re-enabling it would
//!   be a no-op. Surfacing this separately lets the wizard render
//!   "✓ installed" instead of "Available" so the user knows what
//!   they already have on.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// The full output of one scan run. Times are stamped once per run so
/// per-collector `generated_at` doesn't drift if the scan takes a few
/// seconds (it doesn't — scan is sync and fast — but we'd rather have
/// the contract be "one timestamp per report").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanReport {
    pub generated_at: DateTime<Utc>,
    pub platform: Platform,
    pub candidates: Vec<CollectorCandidate>,
}

/// One candidate collector. The `collector_id` is the stable string
/// key (e.g. `"github"`) that matches `CollectorOrchestrator`'s
/// canonical source list (`src-tauri/src/collectors.rs::CANONICAL_SOURCES`)
/// for the three already-implemented sources, plus the seven
/// additional ones onboarding introduces. UI callers key off this
/// field; `display_name` is a human-readable label.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorCandidate {
    pub collector_id: String,
    pub display_name: String,
    pub status: CollectorStatus,
    pub evidence: EvidenceKind,
    pub confidence: f32,
    pub notes: Option<String>,
}

/// Coarse tri-state. See module-level docs for semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorStatus {
    /// Evidence found on disk and not yet enabled in config.
    Available,
    /// No evidence found.
    Unavailable,
    /// Already in the user's loaded `~/.trail/config.json`.
    AlreadyConfigured,
}

/// What we found. The strongest evidence kind wins when multiple
/// are present (e.g. GitHub hosts.yml + gh auth status → keep the
/// command-evidence path because it confirms a logged-in user, not
/// just a stale config file). See `confidence_from_evidence` for
/// the mapping table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EvidenceKind {
    FileExists { path: PathBuf },
    EnvVar { name: String, value: Option<String> },
    DirExists { path: PathBuf },
    CommandExists { binary: String, path: PathBuf },
    MacosAppBundle { path: PathBuf, bundle_id: String },
}

/// Detected host OS. `Other` exists so a future Windows or BSD scan
/// doesn't break the JSON shape — we just round-trip the
/// `cfg(target_os = "...")` string back to the caller.
///
/// Serde representation: externally tagged (the default). Note we do
/// NOT use `#[serde(tag = "os")]` (internally tagged) because that
/// representation explicitly forbids newtype variants (see
/// https://serde.rs/enum-representations.html — "internally tagged"
/// requires every variant to be unit or struct, never tuple/newtype).
/// On non-MacOS / non-Linux builds, `detect_platform()` returns
/// `Platform::Other(os_string)` — a newtype variant — so internally
/// tagged serialization would fail with "cannot serialize tagged
/// newtype variant Platform::Other containing a string" (the exact
/// error that surfaced on Windows during Phase 9 §9.3 onboarding
/// smoke testing). The matching TS type in
/// `src/lib/onboarding/types.ts` mirrors the externally tagged
/// shape: `{ macos: null }` / `{ linux: null }` / `{ other: string }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Platform {
    Macos,
    Linux,
    Other(String),
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Map a status + evidence kind onto the spec's confidence table.
///
/// - FileExists / CommandExists (Available) → 0.95
/// - EnvVar (Available) → 0.80
/// - MacosAppBundle (Available) → 0.90
/// - Unavailable → 0.0 (evidence is a placeholder `FileExists { path: "" }`)
/// - AlreadyConfigured → 1.0 (overrides confidence)
///
/// Centralised so the test cases can pin the mapping + future tweaks
/// land in one place.
fn confidence_for(status: CollectorStatus, evidence: &EvidenceKind) -> f32 {
    match status {
        CollectorStatus::AlreadyConfigured => 1.0,
        CollectorStatus::Unavailable => 0.0,
        CollectorStatus::Available => match evidence {
            EvidenceKind::FileExists { .. } | EvidenceKind::DirExists { .. } => 0.95,
            EvidenceKind::CommandExists { .. } => 0.95,
            EvidenceKind::EnvVar { .. } => 0.80,
            EvidenceKind::MacosAppBundle { .. } => 0.90,
        },
    }
}

/// "We've decided this collector doesn't apply on this OS" placeholder
/// evidence. We never report a phantom file path; instead we expose an
/// empty `FileExists` so the JSON is structurally consistent across
/// platforms but the UX layer can key off `notes` or `confidence == 0.0`
/// to render an explanation.
fn unavailable_evidence() -> EvidenceKind {
    EvidenceKind::FileExists {
        path: PathBuf::new(),
    }
}

/// `FileExists` if `p` exists and is a non-empty regular file (so a
/// zero-byte lock-stub doesn't count as evidence). Returns `None` when
/// the path is absent, a directory, an empty file, or not readable
/// for any reason.
fn probe_file(p: &Path) -> Option<EvidenceKind> {
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_file() || meta.len() == 0 {
        return None;
    }
    Some(EvidenceKind::FileExists {
        path: p.to_path_buf(),
    })
}

/// `DirExists` if `p` exists and is a directory. Symlinks resolve first
/// so a `~/.claude` symlink to a Dropbox folder still counts.
fn probe_dir(p: &Path) -> Option<EvidenceKind> {
    let meta = std::fs::metadata(p).ok()?;
    if !meta.is_dir() {
        return None;
    }
    Some(EvidenceKind::DirExists {
        path: p.to_path_buf(),
    })
}

/// Walk `dir` looking for any `package.json` inside (recursively —
/// real VS Code extensions nest `package.json` at
/// `<ext>/<version>/package.json`). Returns `Some(DirExists)` if any
/// `package.json` is found under `dir`, `None` otherwise.
fn probe_dir_with_package_json(dir: &Path) -> Option<EvidenceKind> {
    walkdir::WalkDir::new(dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .find(|e| {
            e.file_type().is_file()
                && e.file_name() == "package.json"
                && e.path().extension().and_then(|s| s.to_str()) == Some("json")
        })
        .map(|_| probe_dir(dir))?
}

/// Run `gh auth status` and return the `CommandExists` path if exit 0.
/// We never error-out if gh isn't installed or the user isn't logged
/// in — the github evidence falls back to file-existence alone (see
/// `scan_github` below), which matches the spec's Privacy rule.
fn probe_gh_auth() -> Option<EvidenceKind> {
    let output = std::process::Command::new("gh")
        .args(["auth", "status"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // Resolve the `gh` binary path so the JSON points at something
    // concrete. `which::which` would be lighter but adding a dep just
    // for a string is overkill — if `Command::new("gh")` succeeded,
    // the executable is on PATH at the env var captured below.
    let path = std::env::var_os("PATH")
        .map(PathBuf::from)
        .unwrap_or_default();
    let resolved = std::env::split_paths(&path)
        .map(|d| d.join("gh"))
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("gh"));
    Some(EvidenceKind::CommandExists {
        binary: "gh".to_string(),
        path: resolved,
    })
}

/// Run `gh auth status` and return the `CommandExists` path if exit 0.
/// Resolve `$HOME` (or fall back to a tempdir-friendly placeholder so
/// tests with `HOME=` empties don't crash with `path::join` panics).
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Detect platform. Pure — no IO.
fn detect_platform() -> Platform {
    if cfg!(target_os = "macos") {
        Platform::Macos
    } else if cfg!(target_os = "linux") {
        Platform::Linux
    } else {
        Platform::Other(std::env::consts::OS.to_string())
    }
}

/// Wrap the per-collector probe result into a `CollectorCandidate`.
/// Handles the "unavailable" case so the 8 callers below stay
/// one-liners.
fn finalize(
    collector_id: &str,
    display_name: &str,
    status: CollectorStatus,
    evidence: EvidenceKind,
    notes: Option<String>,
) -> CollectorCandidate {
    let confidence = confidence_for(status, &evidence);
    CollectorCandidate {
        collector_id: collector_id.to_string(),
        display_name: display_name.to_string(),
        status,
        evidence,
        confidence,
        notes,
    }
}

/// Returns the per-collector List of *"is the id already configured?"*
/// evaluator. We check `pending_installs` as the source of truth
/// because the app's `Config` uses that field as the "collectors
/// already installed" registry (see `src-tauri/src/config.rs`). When
/// the config file is missing we report nothing as
/// "already-configured", which is the correct behaviour — first run.
/// `path` lets tests inject a temp config path.
fn already_configured_ids(config_path: &Path) -> Vec<String> {
    let Ok(cfg) = crate::config::load_config(config_path) else {
        return Vec::new();
    };
    cfg.pending_installs
}

// ---------------------------------------------------------------------------
// Public entry points
// ---------------------------------------------------------------------------

/// Run all 8 probes synchronously and return a fully-populated
/// `ScanReport`. This is sync on purpose — each probe is either a
/// single `metadata()` syscall or a `gh auth status` subprocess; the
/// whole scan finishes in <100 ms on a stock macbook. If we ever
/// need to scale up, the per-collector probes are pure functions of
/// `home + configured_ids` and easy to fan out via `tokio::join!`
/// later.
///
/// `config_path` is the optional on-disk `~/.trail/config.json`
/// path; `None` means "treat as first-run, no collectors are
/// already-configured". Tests inject a temp file so they can
/// exercise the `AlreadyConfigured` arm deterministically.
pub fn scan_laptop() -> ScanReport {
    let platform = detect_platform();
    let home = home_dir();
    // Default: the prod location. `LIB` callers (tests) override via
    // `scan_laptop_with_config`.
    let config_path = home.join(".trail").join("config.json");
    scan_laptop_with_config(&platform, &home, &config_path)
}

/// Same as [`scan_laptop`] but with `config_path` injectable so tests
/// can stage a temp `~/.trail/config.json` and exercise the
/// `AlreadyConfigured` arm without polluting the real home dir.
pub fn scan_laptop_with_config(platform: &Platform, home: &Path, config_path: &Path) -> ScanReport {
    let configured = already_configured_ids(config_path);
    let mark_configured = |candidate: &mut CollectorCandidate| {
        if configured.iter().any(|id| id == &candidate.collector_id) {
            candidate.status = CollectorStatus::AlreadyConfigured;
            candidate.confidence = 1.0;
        }
    };

    let mut candidates = Vec::with_capacity(8);
    let mut c = scan_github(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_calendar(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_claude_sessions(home);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_gmail(platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_notes(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_vscode_extensions(home);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_chrome_history(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_brave_history(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    ScanReport {
        generated_at: Utc::now(),
        platform: platform.clone(),
        candidates,
    }
}

// ---------------------------------------------------------------------------
// Per-collector probes
// ---------------------------------------------------------------------------

/// GitHub: `~/.config/gh/hosts.yml` exists AND `gh auth status`
/// reports logged-in. We prefer the `CommandExists` evidence when
/// both succeed because it confirms a live logged-in user (vs. a
/// stale hosts.yml from a previous host).
///
/// Privacy: never read the file contents — just `metadata()`. The
/// `gh auth status` invocation only tells us "logged in: yes/no".
/// We do not capture the GitHub account name or token.
fn scan_github(home: &Path, _platform: &Platform) -> CollectorCandidate {
    let hosts_yml = home.join(".config").join("gh").join("hosts.yml");
    let file_evidence = probe_file(&hosts_yml);
    let auth_evidence = probe_gh_auth();
    let (status, evidence, notes) = match (file_evidence, auth_evidence) {
        (Some(_), Some(cmd)) => (CollectorStatus::Available, cmd, None),
        (Some(file), None) => (
            CollectorStatus::Available,
            file,
            Some("gh config file present but `gh auth status` non-zero".to_string()),
        ),
        (None, Some(cmd)) => (CollectorStatus::Available, cmd, None),
        (None, None) => (
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("no ~/.config/gh/hosts.yml and `gh auth status` not logged in".to_string()),
        ),
    };
    finalize("github", "GitHub activity", status, evidence, notes)
}

/// Calendar: macOS Calendar.app + the `~/Library/Calendars/*.ics`
/// saved-state files. Linux fallback: `~/.config/evolution/` or
/// `~/.local/share/evolution/` (GNOME Evolution). On other OSes we
/// report Unavailable with a `notes` explaining the platform skip.
///
/// 2026-08-11 — added the EventKit TCC probe (when the
/// `calendar_event_kit_tcc` feature is enabled, macOS only).
/// The probe calls `EKEventStore.authorizationStatusForEntityType`
/// to read the current TCC state without prompting. We
/// return `Available` with strong evidence on
/// `EKAuthorizationStatusFullAccess` (Sonoma+), `Available`
/// with a "Run the wizard" note on `.notDetermined` (the
/// user hasn't seen the TCC dialog yet), and `Unavailable`
/// with a "denied" note on `.denied` /
/// `.writeOnlyAccessDenied` / `.restricted`. The probe is
/// the authoritative answer; the saved-state and
/// Calendar.app bundle probes are the fallbacks when
/// EventKit is not available (or the user is on Linux).
fn scan_calendar(home: &Path, platform: &Platform) -> CollectorCandidate {
    match platform {
        Platform::Macos => {
            // 2026-08-11 — prefer the EventKit TCC probe when
            // it's available. The probe reads
            // `EKEventStore.authorizationStatusForEntityType`
            // which does NOT trigger a TCC dialog (it's
            // read-only). The returned `EKAuthorizationStatus`
            // is the same value the wizard reads later, so
            // the scan and the wizard agree on the user's
            // permission state.
            //
            // The probe + enum are `#[cfg(target_os = "macos")]`-gated
            // — on Linux the function is absent entirely, so
            // this branch is also `#[cfg]`-gated. The dispatch
            // for Linux is the existing evolution path below.
            #[cfg(target_os = "macos")]
            {
                if let Some(tcc_state) = calendar_eventkit_tcc_status() {
                    match tcc_state {
                        CalendarEventKitTcc::FullAccess => {
                            return finalize(
                                "calendar",
                                "Calendar events",
                                CollectorStatus::Available,
                                EvidenceKind::MacosAppBundle {
                                    path: PathBuf::from("/Applications/Calendar.app"),
                                    bundle_id: "com.apple.iCal".to_string(),
                                },
                                Some(
                                    "EventKit full-calendar access granted; \
                                     Calendar.app is ready to read"
                                        .to_string(),
                                ),
                            );
                        }
                        CalendarEventKitTcc::NotDetermined => {
                            return finalize(
                                "calendar",
                                "Calendar events",
                                CollectorStatus::Available,
                                EvidenceKind::MacosAppBundle {
                                    path: PathBuf::from("/Applications/Calendar.app"),
                                    bundle_id: "com.apple.iCal".to_string(),
                                },
                                Some(
                                    "EventKit permission not yet requested; \
                                     run the wizard to enable Calendar \
                                     capture (System Settings → Privacy \
                                     → Calendars)"
                                        .to_string(),
                                ),
                            );
                        }
                        CalendarEventKitTcc::Denied => {
                            return finalize(
                                "calendar",
                                "Calendar events",
                                CollectorStatus::Unavailable,
                                EvidenceKind::FileExists { path: PathBuf::new() },
                                Some(
                                    "EventKit access denied. Open System \
                                     Settings → Privacy & Security → \
                                     Calendars and grant Trail full access"
                                        .to_string(),
                                ),
                            );
                        }
                    }
                }
            }
            // Fallback: the saved-state probe. The
            // `~/Library/Calendars/Calendar.savedState`
            // directory only exists after Calendar.app has
            // been launched at least once, so a fresh
            // install may legitimately not have it.
            let saved = home
                .join("Library")
                .join("Calendars")
                .join("Calendar.savedState");
            if let Some(ev) = probe_dir(saved.parent().unwrap_or(&saved)) {
                return finalize(
                    "calendar",
                    "Calendar events",
                    CollectorStatus::Available,
                    ev,
                    None,
                );
            }
            // Fallback: a bundle-id reference is the weaker evidence
            // but still means "Calendar.app is installed". We don't
            // shell out to `mdfind` here because that adds 50-200 ms
            // and the spec accepts "app installed" as 0.90 conf.
            let bundle_path = PathBuf::from("/Applications/Calendar.app");
            if bundle_path.is_dir() {
                return finalize(
                    "calendar",
                    "Calendar events",
                    CollectorStatus::Available,
                    EvidenceKind::MacosAppBundle {
                        path: bundle_path,
                        bundle_id: "com.apple.iCal".to_string(),
                    },
                    None,
                );
            }
            finalize(
                "calendar",
                "Calendar events",
                CollectorStatus::Unavailable,
                unavailable_evidence(),
                Some("macOS Calendar.app not detected".to_string()),
            )
        }
        Platform::Linux => {
            let candidates = [
                home.join(".config").join("evolution"),
                home.join(".local").join("share").join("evolution"),
            ];
            for dir in &candidates {
                if let Some(ev) = probe_dir(dir) {
                    return finalize(
                        "calendar",
                        "Calendar events",
                        CollectorStatus::Available,
                        ev,
                        None,
                    );
                }
            }
            finalize(
                "calendar",
                "Calendar events",
                CollectorStatus::Unavailable,
                unavailable_evidence(),
                Some("no evolution calendar store found".to_string()),
            )
        }
        Platform::Other(os) => finalize(
            "calendar",
            "Calendar events",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some(format!("calendar collector not yet supported on {os}")),
        ),
    }
}

/// The tri-state answer the EventKit TCC probe returns. Mirrors
/// `EKAuthorizationStatus` from the EventKit framework. The
/// values are documented in Apple's
/// `EKAuthorizationStatus` enum reference.
///
/// The enum is `#[cfg(target_os = "macos")]`-gated along with
/// the probe function. The call site (`scan_calendar`)
/// pattern-matches on the probe's return type which is
/// `Option<CalendarEventKitTcc>` only on macOS — on Linux the
/// function is absent entirely (the dispatch in
/// `scan_calendar` is `#[cfg]`'d accordingly). See
/// `calendar_eventkit_tcc_status` below for the macOS-side
/// implementation.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CalendarEventKitTcc {
    /// `.fullAccess` (Sonoma+) or legacy `.authorized`.
    /// The collector can read all events.
    FullAccess,
    /// `.notDetermined`. The user has never seen the TCC
    /// dialog. The wizard's first capture will trigger it.
    NotDetermined,
    /// `.denied` / `.writeOnlyAccessDenied` / `.restricted`.
    /// The collector cannot read events; the user must
    /// change System Settings.
    Denied,
}

/// Read-only EventKit TCC probe. Returns `None` when EventKit
/// is not available (non-macOS, or the objc2 binding isn't
/// compiled in) so the caller can fall through to the
/// filesystem-based probes. The probe does NOT prompt the
/// user — `authorizationStatusForEntityType:` is a cheap
/// read-only TCC query.
///
/// `#[cfg(target_os = "macos")]` gates the function and the
/// return-type enum together. On non-macOS targets, the
/// function is `unreachable!()` (the call site in
/// `scan_calendar` is also `#[cfg]`-gated, so this branch
/// is dead code at runtime). The `Option<CalendarEventKitTcc>`
/// signature on the non-macOS arm intentionally fails to
/// compile — it's a phantom that catches any future
/// refactor that tries to call the probe from a non-macOS
/// call site.
#[cfg(target_os = "macos")]
fn calendar_eventkit_tcc_status() -> Option<CalendarEventKitTcc> {
    // The TCC probe lives in `voice/permission.rs` (a sibling
    // module) — the same `objc2` + `EKEventStore` plumbing
    // applies here. We re-implement the probe inline (rather
    // than refactoring `voice/permission.rs` into a generic
    // "macOS TCC probe" helper) because the surface is small
    // and the two callers have different enum mappings.
    use objc2::{class, msg_send};
    use objc2_event_kit::EKAuthorizationStatus;
    unsafe {
        // `EKEventStore` is registered at process load (it's
        // a class in `EventKit.framework`, linked via the
        // crate's Cargo.toml + build.rs). The `class!` macro
        // returns a non-null `&'static AnyClass` — objc2 0.6
        // dropped the typed wrapper's `is_null` method, so we
        // rely on the macro's invariant. Same convention as
        // `voice/permission.rs::authorization_status`.
        // `+authorizationStatusForEntityType:` is a class
        // method that returns `EKAuthorizationStatus`
        // (an NSInteger enum). The argument is the entity
        // type — `.event` (value 0) for calendars. objc2
        // exposes the constants as associated constants on
        // `EKAuthorizationStatus` (`FullAccess`, `Denied`,
        // etc., each of which is `Self(N)` for the historical
        // Apple enum value). We pattern-match the `status`
        // value directly — no inner-construction needed.
        let status: EKAuthorizationStatus =
            msg_send![class!(EKEventStore), authorizationStatusForEntityType: 0isize];
        match status {
            EKAuthorizationStatus::FullAccess => Some(CalendarEventKitTcc::FullAccess),
            EKAuthorizationStatus::Authorized => Some(CalendarEventKitTcc::FullAccess),
            EKAuthorizationStatus::NotDetermined => Some(CalendarEventKitTcc::NotDetermined),
            EKAuthorizationStatus::Denied
            | EKAuthorizationStatus::Restricted
            | EKAuthorizationStatus::WriteOnly => Some(CalendarEventKitTcc::Denied),
            _ => None,
        }
    }
}

/// Claude sessions: any of `~/.claude/projects/` or `~/.claude/sessions/`.
/// File evidence (DirExists) only — we never peek inside.
fn scan_claude_sessions(home: &Path) -> CollectorCandidate {
    let projects = home.join(".claude").join("projects");
    let sessions = home.join(".claude").join("sessions");
    let ev = probe_dir(&projects)
        .or_else(|| probe_dir(&sessions))
        .unwrap_or_else(unavailable_evidence);
    let status = if matches!(ev, EvidenceKind::DirExists { .. }) {
        CollectorStatus::Available
    } else {
        CollectorStatus::Unavailable
    };
    finalize(
        "claude_sessions",
        "Claude sessions",
        status,
        ev,
        if status == CollectorStatus::Unavailable {
            Some("no ~/.claude/projects/ or ~/.claude/sessions/".to_string())
        } else {
            None
        },
    )
}

/// Gmail: macOS Mail.app is the host that surfaces IMAP to the
/// scanner. We do NOT touch any token or OAuth state — just the
/// bundle-id is enough to mark "available". Falls through to
/// Unavailable on non-macOS per the spec.
fn scan_gmail(platform: &Platform) -> CollectorCandidate {
    match platform {
        Platform::Macos => {
            let mail_app = PathBuf::from("/Applications/Mail.app");
            if mail_app.is_dir() {
                finalize(
                    "gmail",
                    "Gmail (via Apple Mail)",
                    CollectorStatus::Available,
                    EvidenceKind::MacosAppBundle {
                        path: mail_app,
                        bundle_id: "com.apple.mail".to_string(),
                    },
                    Some("evidence-only; OAuth scan is the wizard's job".to_string()),
                )
            } else {
                finalize(
                    "gmail",
                    "Gmail (via Apple Mail)",
                    CollectorStatus::Unavailable,
                    unavailable_evidence(),
                    Some("macOS Mail.app not installed".to_string()),
                )
            }
        }
        _ => finalize(
            "gmail",
            "Gmail (via Apple Mail)",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("gmail collector is macOS-only".to_string()),
        ),
    }
}

/// Notes: Apple Notes on macOS, `notes-go` flat-file store on Linux.
/// File evidence, no reads.
fn scan_notes(home: &Path, platform: &Platform) -> CollectorCandidate {
    match platform {
        Platform::Macos => {
            let container = home
                .join("Library")
                .join("Group Containers")
                .join("group.com.apple.notes");
            if let Some(ev) = probe_dir(&container) {
                return finalize("notes", "Notes", CollectorStatus::Available, ev, None);
            }
            finalize(
                "notes",
                "Notes",
                CollectorStatus::Unavailable,
                unavailable_evidence(),
                Some("Apple Notes container not found".to_string()),
            )
        }
        Platform::Linux => {
            let notes_go = home.join(".local").join("share").join("notes-go");
            if let Some(ev) = probe_dir(&notes_go) {
                return finalize("notes", "Notes", CollectorStatus::Available, ev, None);
            }
            finalize(
                "notes",
                "Notes",
                CollectorStatus::Unavailable,
                unavailable_evidence(),
                Some("notes-go store not present".to_string()),
            )
        }
        Platform::Other(os) => finalize(
            "notes",
            "Notes",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some(format!("notes collector not yet supported on {os}")),
        ),
    }
}

/// VS Code extensions: `~/.vscode/extensions/` OR
/// `~/.vscode-insiders/extensions/`. We only report it as Available
/// if the dir contains at least one `package.json` (the marker that
/// distinguishes a populated extensions folder from an empty stub).
fn scan_vscode_extensions(home: &Path) -> CollectorCandidate {
    for dir in [
        home.join(".vscode").join("extensions"),
        home.join(".vscode-insiders").join("extensions"),
    ] {
        if let Some(ev) = probe_dir_with_package_json(&dir) {
            return finalize(
                "vscode_extensions",
                "VS Code extensions",
                CollectorStatus::Available,
                ev,
                None,
            );
        }
    }
    finalize(
        "vscode_extensions",
        "VS Code extensions",
        CollectorStatus::Unavailable,
        unavailable_evidence(),
        Some("no ~/.vscode/extensions/ or ~/.vscode-insiders/extensions/".to_string()),
    )
}

/// Chrome history: `~/Library/Application Support/Google/Chrome/Default/History`
/// on macOS, `~/.config/google-chrome/Default/History` on Linux.
/// **NEVER read the file** — only check existence + size > 0. The
/// collector that reads the SQLite runs later, after the user
/// explicitly opts in (item 6-2).
fn scan_chrome_history(home: &Path, platform: &Platform) -> CollectorCandidate {
    let path = chrome_brave_history_path(home, platform, "google-chrome");
    probe_history_file(path, "chrome_history", "Chrome history")
}

/// Brave history: same shape as Chrome, different vendor dir.
fn scan_brave_history(home: &Path, platform: &Platform) -> CollectorCandidate {
    let path = chrome_brave_history_path(home, platform, "BraveSoftware/Brave-Browser");
    probe_history_file(path, "brave_history", "Brave history")
}

/// Shared helper: resolve the per-OS path, probe, finalize. Kept
/// private — the public API is the `scan_*` function per collector.
fn chrome_brave_history_path(home: &Path, platform: &Platform, vendor: &str) -> PathBuf {
    match platform {
        Platform::Macos => home
            .join("Library")
            .join("Application Support")
            .join("Google")
            .join("Chrome")
            .join("Default")
            .join("History")
            // The vendor argument is ignored on macOS (both Chrome + Brave
            // land under `Application Support/<Vendor>`); we keep the
            // function shape matching the Linux branch so it stays a
            // pure function of `(home, platform, vendor)`.
            .pipe(|p| match vendor {
                "BraveSoftware/Brave-Browser" => home
                    .join("Library")
                    .join("Application Support")
                    .join("BraveSoftware")
                    .join("Brave-Browser")
                    .join("Default")
                    .join("History"),
                _ => p,
            }),
        Platform::Linux => home
            .join(".config")
            .join(vendor)
            .join("Default")
            .join("History"),
        Platform::Other(_) => PathBuf::new(),
    }
}

/// Tiny `Pipe` trait for the macOS-redirect branch above. Anything
/// that fits in a closure: `p.pipe(|x| ...)`. Avoids pulling in the
/// `pipe-trait` crate for one local call site.
trait Pipe<T> {
    fn pipe<U>(self, f: impl FnOnce(T) -> U) -> U;
}
impl<T> Pipe<T> for T {
    fn pipe<U>(self, f: impl FnOnce(T) -> U) -> U {
        f(self)
    }
}

fn probe_history_file(path: PathBuf, id: &str, display_name: &str) -> CollectorCandidate {
    if path.as_os_str().is_empty() {
        return finalize(
            id,
            display_name,
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("platform not supported".to_string()),
        );
    }
    if let Some(ev) = probe_file(&path) {
        return finalize(id, display_name, CollectorStatus::Available, ev, None);
    }
    finalize(
        id,
        display_name,
        CollectorStatus::Unavailable,
        unavailable_evidence(),
        Some(format!("{} not present", path.display())),
    )
}

// ---------------------------------------------------------------------------
// Tauri command
// ---------------------------------------------------------------------------

/// Tauri command wrapper for `scan_laptop`. Exposed so the
/// frontend's onboarding wizard (item 6-4) can invoke it as
/// `invoke('scan_laptop_cmd')` without holding its own copy of
/// the probe logic.
///
/// The `home` parameter lets the wizard override `$HOME` for
/// re-scanning after `~/.trail/` is first created; left as an
/// optional second arg once we add it in 6-4.
#[tauri::command]
pub fn scan_laptop_cmd() -> ScanReport {
    scan_laptop()
}

// ---------------------------------------------------------------------------
// Tests (unit)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    /// Build a tempdir with the home directory positioned at `$HOME`
    /// for the duration of the test. Returns the tempdir handle so
    /// RAII drops it on test exit.
    struct TempHome(PathBuf);
    impl TempHome {
        fn new() -> Self {
            let td = tempfile::tempdir().expect("tempdir");
            std::env::set_var("HOME", td.path());
            Self(td.path().to_path_buf())
        }
        fn path(&self) -> &Path {
            &self.0
        }
        fn touch(&self, rel: &str) {
            let p = self.0.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, b"x").unwrap();
        }
        fn mkdir(&self, rel: &str) {
            std::fs::create_dir_all(self.0.join(rel)).unwrap();
        }
        fn write_config(&self, rel: &str, body: &str) -> PathBuf {
            self.touch(rel);
            let p = self.0.join(rel);
            std::fs::write(&p, body).unwrap();
            p
        }
    }
    impl Drop for TempHome {
        fn drop(&mut self) {
            // Best-effort: clear $HOME so concurrent tests don't
            // inherit a stale value. We don't unset — `set_var("HOME",
            // "")` is what cargo test rigs use to avoid panic-on-unset.
            std::env::set_var("HOME", "");
        }
    }

    fn find<'a>(report: &'a ScanReport, id: &str) -> &'a CollectorCandidate {
        report
            .candidates
            .iter()
            .find(|c| c.collector_id == id)
            .unwrap_or_else(|| panic!("missing candidate {id}"))
    }

    #[test]
    fn scan_report_has_generated_at_and_platform() {
        let home = TempHome::new();
        // Race-safe: pass the tempdir as `home` explicitly so
        // concurrent TempHome tests don't fight over $HOME.
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        assert!(report.generated_at.timestamp() > 0);
        assert!(matches!(report.platform, Platform::Linux | Platform::Macos));
        // The 8 collectors are always present so the wizard never
        // sees a "missing row" — explicit `Unavailable` instead.
        for id in [
            "github",
            "calendar",
            "claude_sessions",
            "gmail",
            "notes",
            "vscode_extensions",
            "chrome_history",
            "brave_history",
        ] {
            assert!(report.candidates.iter().any(|c| c.collector_id == id));
        }
    }

    #[test]
    fn github_evidence_from_gh_config_file() {
        let home = TempHome::new();
        home.touch(".config/gh/hosts.yml");
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let g = find(&report, "github");
        // On a CI host `gh auth status` may not be logged in, so the
        // scan falls back to FileExists per the spec.
        match (&g.status, &g.evidence) {
            (CollectorStatus::Available, EvidenceKind::FileExists { path }) => {
                assert!(path.ends_with("hosts.yml"));
            }
            (CollectorStatus::Available, EvidenceKind::CommandExists { binary, .. }) => {
                assert_eq!(binary, "gh");
            }
            _ => panic!("unexpected github candidate: {:?}", g),
        }
        assert!(g.confidence > 0.7);
    }

    #[test]
    fn github_unavailable_when_no_gh_config() {
        let home = TempHome::new();
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let g = find(&report, "github");
        assert_eq!(g.status, CollectorStatus::Unavailable);
        assert_eq!(g.confidence, 0.0);
    }

    #[test]
    fn claude_sessions_evidence_from_dot_claude_dir() {
        let home = TempHome::new();
        home.mkdir(".claude/projects/work");
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let c = find(&report, "claude_sessions");
        assert_eq!(c.status, CollectorStatus::Available);
        match &c.evidence {
            EvidenceKind::DirExists { path } => assert!(path.ends_with("projects")),
            other => panic!("expected DirExists, got {other:?}"),
        }
        assert!((c.confidence - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn vscode_extensions_evidence_from_extensions_dir() {
        let home = TempHome::new();
        home.mkdir(".vscode/extensions/some-ext/1.0.0");
        home.touch(".vscode/extensions/some-ext/1.0.0/package.json");
        // Use the explicit-home entrypoint so we don't race against
        // any other test that happens to set $HOME concurrently.
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let v = find(&report, "vscode_extensions");
        assert_eq!(v.status, CollectorStatus::Available);
        assert!(matches!(v.evidence, EvidenceKind::DirExists { .. }));
    }

    #[test]
    fn macos_only_collectors_skipped_on_linux() {
        let home = TempHome::new();
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        // On Linux, gmail (mac-only) must be Unavailable with a
        // "macos-only"-style note. Calendar falls through to the
        // Linux evolution fallback, so we don't assert on it here.
        let g = find(&report, "gmail");
        assert_eq!(g.status, CollectorStatus::Unavailable);
        assert!(g
            .notes
            .as_deref()
            .map(|n| n.contains("macOS") || n.contains("mac"))
            .unwrap_or(false));
    }

    #[test]
    fn chrome_history_evidence_path_correct_per_platform() {
        let home = TempHome::new();
        // Linux branch: `$HOME/.config/google-chrome/Default/History`.
        home.touch(".config/google-chrome/Default/History");
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let c = find(&report, "chrome_history");
        assert_eq!(c.status, CollectorStatus::Available);
        match &c.evidence {
            EvidenceKind::FileExists { path } => {
                assert!(path.ends_with("google-chrome/Default/History"));
            }
            _ => panic!("expected FileExists for chrome history"),
        }
    }

    #[test]
    fn already_configured_overrides_available() {
        let home = TempHome::new();
        home.touch(".config/gh/hosts.yml");
        // Pre-populate ~/.trail/config.json with `pending_installs:
        // ["github"]` so the orchestrator already considers github
        // installed.
        let cfg = home.write_config(
            ".trail/config.json",
            r#"{
                "claude_sessions_paths": [],
                "github": {"mode":"gh_cli","host":"github.com"},
                "calendar_ics":"x",
                "calendar":{"kind":"ics","path":"x"},
                "voice":{"enabled":false,"hotkey":"x","transcriber":"x","model":"x"},
                "review_time":"18:00",
                "summarizer":{"model":"x","model_provider":"local","anonymization_strictness":"aggressive","use_generic_categories":false},
                "transport":{"type":"ssh","host":"x","port":22,"user":"u","auth":{"auth":"public_key","path":"/tmp/x"},"remote_path":"/tmp/x"},
                "raw_retention_days":7,
                "pending_installs":["github"]
            }"#,
        );
        let platform = detect_platform();
        let report = scan_laptop_with_config(&platform, home.path(), &cfg);
        let g = find(&report, "github");
        assert_eq!(g.status, CollectorStatus::AlreadyConfigured);
        assert_eq!(g.confidence, 1.0);
    }

    #[test]
    fn empty_evidence_treated_as_unavailable() {
        let home = TempHome::new();
        home.mkdir(".vscode/extensions"); // empty dir, no package.json
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let v = find(&report, "vscode_extensions");
        assert_eq!(
            v.status,
            CollectorStatus::Unavailable,
            "empty extensions dir must not be reported as available"
        );
    }
}
