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
//! ## Per-mail-client calendar detectors
//!
//! In addition to the per-source `CollectorCandidate` rows above,
//! `scan_evolution_calendars` walks the user's Evolution calendar
//! store and returns a `Vec<DetectedCalendar>` — one entry per
//! discovered `.ics` file. The orchestrator surfaces the count
//! inside the `calendar` candidate's `notes` (e.g. "auto-discovered
//! 3 calendars") so the wizard UI can show a richer hint; the typed
//! `Vec<DetectedCalendar>` itself is consumed by the LLM step
//! (`llm.rs`, Phase B) and rendered as a multi-select picker.
//!
//! `scan_gnome_calendar_calendars` is the alias detector for users
//! who install only the GNOME Calendar GUI (no Evolution MUA). It
//! walks the same on-disk roots as `scan_evolution_calendars`
//! (because GNOME Calendar piggybacks on evolution-data-server) but
//! emits each `.ics` with `client = "gnome_calendar"` so the Ask step
//! renders the right label for those users. Two heuristics gate the
//! emission: `gnome-calendar` must be on the user's PATH, and
//! `evolution` must NOT be (when Evolution is installed, ECD-1
//! already labels the entries). The ics_path dedup is a defensive
//! safety net — the heuristic gates are the authoritative control.
//!
//! The pattern mirrors the per-detector shape used by
//! `scan_chrome_history` / `scan_firefox_history`: pure function of
//! `(home, platform)`, platform-aware so non-Linux targets return
//! empty.
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
use std::sync::Mutex;

// ---------------------------------------------------------------------------
// ECD-2 — GNOME Calendar alias heuristic mocks
// ---------------------------------------------------------------------------
//
// The "is `gnome-calendar` installed?" / "is `evolution` installed?"
// heuristics are environment-dependent (they shell out to `which`),
// which makes them hard to test from a deterministic unit suite. We
// mirror the `install.rs` thread-local mock seam pattern (see
// `INVOKE_INSTALL_SCRIPT` / `set_install_script_invoker`): the
// heuristic functions consult a `Mutex<Option<bool>>` slot; in
// production the slot is `None` and the function falls through to a
// `which`-style probe; in tests the slot is `Some(value)` and the
// function returns that value verbatim.
//
// The slot is a `Mutex<Option<bool>>` rather than an `AtomicBool` so
// the tests' `with_heuristics(...)` helper can save + restore the
// previous value across a body via an RAII guard (panic-safe). When
// no mock is set, the production code runs `which gnome-calendar`
// (or `which evolution`) — no `which` crate, just
// `std::process::Command` per the no-new-dep rule.

/// Test-only mock slot for `is_gnome_calendar_installed_for`. `None`
/// in production — fall through to the `which` probe. `Some(b)` in
/// tests — return `b` verbatim.
static GNOME_CALENDAR_PRESENT: Mutex<Option<bool>> = Mutex::new(None);

/// Test-only mock slot for `is_evolution_installed_for`. Same shape
/// as `GNOME_CALENDAR_PRESENT`.
static EVOLUTION_PRESENT: Mutex<Option<bool>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// ECD-4 — KOrganizer + Outlook UX-fallback mocks
// ---------------------------------------------------------------------------
//
// Same shape as the ECD-2 heuristic mocks: `None` in production
// (falls through to a `which`-style probe on Linux for KOrganizer,
// or a `Path::exists` probe on `%ProgramFiles%\\Microsoft
// Office\\root\\Office16\\OUTLOOK.EXE` for Outlook), `Some(b)` in
// tests (returns the value verbatim). The non-target platform
// short-circuit (`is_korganizer_installed_for` only consults the
// probe on Linux; `is_outlook_installed_for` only on Windows)
// runs BEFORE the mock is checked, so tests that stage a
// `Platform::Linux` + `KOrganizer_PRESENT = Some(true)` always
// see `true` regardless of whether `korganizer` is actually
// installed in the test environment.

/// Test-only mock slot for `is_korganizer_installed_for`. `None`
/// in production — fall through to the `which korganizer` probe.
/// `Some(b)` in tests — return `b` verbatim.
static KORGANIZER_PRESENT: Mutex<Option<bool>> = Mutex::new(None);

/// Test-only mock slot for `is_outlook_installed_for`. Same shape
/// as `KORGANIZER_PRESENT`.
static OUTLOOK_PRESENT: Mutex<Option<bool>> = Mutex::new(None);

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

/// One calendar discovered by a per-mail-client detector. Returned
/// by [`scan_evolution_calendars`] (and, in a follow-up, by the
/// Thunderbird/KOrganizer/Outlook detectors proposed in
/// `2026-08-14_email-calendar-discovery-proposal.md`). The
/// orchestrator layer above consumes these and the LLM step (Phase
/// B, `llm.rs`) surfaces them as a multi-select picker in
/// `StepAsk.svelte`.
///
/// The struct deliberately does NOT depend on [`CollectorCandidate`]:
/// the per-detector scan is a typed value, not a `CollectorCandidate`
/// row — the wiring that turns a `Vec<DetectedCalendar>` into
/// `ScanReport`-shaped metadata is left to the higher-level
/// orchestrator (and lives in the future ECD-2/ECD-3 PRs).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedCalendar {
    /// Stable identifier of the mail/calendar client that produced
    /// this detection. Values in use: `"evolution"` (the on-disk
    /// `.ics` files under `~/.local/share/evolution/calendar/...`)
    /// and `"gnome_calendar"` (the alias detector for users who
    /// install only the GNOME Calendar GUI; same on-disk files,
    /// distinct user-facing label). Future per-client detectors
    /// (`"thunderbird"`, `"korganizer"`, `"outlook"`) will land in
    /// ECD-3+. Matches the `client` column proposed for the new
    /// `email_calendar_candidates` field in the answers schema
    /// (proposal §"Architecture sketch").
    pub client: String,
    /// Human-readable profile identifier for the source. For
    /// Evolution this is the `<source>` directory name
    /// (e.g. `"On This Computer"`, `"Google"`, `"CalDAV — \
    /// work@example.com"`) optionally combined with the per-source
    /// email when the `~/.config/evolution/sources/*.source` JSON
    /// can be parsed. `None` when the source has no `.source` file
    /// or the JSON is malformed (the detector still returns the
    /// calendar — the profile label is just less rich).
    pub profile: Option<String>,
    /// Per-calendar display name extracted from the `.ics`'s
    /// `X-EVOLUTION-CALENDAR` property. `None` when the property
    /// is absent (a fresh, never-touched Evolution install writes
    /// `.ics` files without the property until the user opens the
    /// calendar in the GUI).
    pub display_name: Option<String>,
    /// Absolute path to the `.ics` file. This is the path the
    /// collector (`crates/trail-collector/src/collectors/calendar/ical.rs`)
    /// later reads; the scanner never reads it during onboarding.
    pub ics_path: PathBuf,
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
    let mut c = scan_claude_sessions(home);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_github(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_calendar(home, platform);
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

    // Browser-history probes. The order here MUST match
    // `StepAsk.svelte`'s answer-row order — the user sees
    // the same set of rows in the same order on both
    // Step 2 (scan findings) and Step 3 (answers). The
    // future history collector that reads these files will
    // share the same order via a `BrowserSource` enum on
    // `CollectorLaptopConfig` (mirroring `CalendarSource`).
    c = scan_chrome_history(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_brave_history(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_firefox_history(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_opera_history(home, platform);
    mark_configured(&mut c);
    candidates.push(c);

    c = scan_safari_history(home, platform);
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

/// Evolution (GNOME) calendar detector. Walks
/// `~/.local/share/evolution/calendar/<source>/*.ics` AND
/// `~/.config/evolution/calendar/<source>/*.ics` (Evolution's
/// XDG-compliant data layout keeps the calendar blobs under
/// `XDG_DATA_HOME` and the source metadata under `XDG_CONFIG_HOME`).
/// Each `.ics` file becomes one [`DetectedCalendar`].
///
/// Per proposal §"Per-detector implementation notes → Evolution":
/// - `<source>` is the directory name; the system account
///   (`local@*/`) is recognised but its calendars are only
///   emitted when the source directory holds at least one
///   non-empty `.ics` (Evolution ships a `local@local-…` stub on
///   a fresh install that points at an empty `system-calendar.ics`;
///   we skip stubs and only emit real ones).
/// - `X-EVOLUTION-CALENDAR` is the per-calendar display name
///   property Evolution writes when a calendar is opened in the
///   GUI. We parse it with a tiny inline line-fold parser (iCal
///   line folding is "any line beginning with whitespace is a
///   continuation of the previous line" — RFC 5545 §3.1).
/// - When `~/.config/evolution/sources/<source-uid>.source` is a
///   valid JSON file with a `parent[1].text` field, we append
///   that email to the profile label (e.g.
///   `"CalDAV — work@example.com"`). Malformed `.source` files
///   are ignored — we still return the calendar, the profile
///   just lacks the email.
///
/// `home` is the user's home dir (the caller resolves `$HOME` or
/// passes a test fixture); `platform` is the runtime-detected
/// [`Platform`] so the function returns empty on non-Linux
/// targets (the test seam matches the per-detector pattern used
/// by `scan_chrome_history`/`scan_firefox_history`). The function
/// is a pure read-walk — no Mutex, no OnceLock.
pub fn scan_evolution_calendars(home: &Path, platform: &Platform) -> Vec<DetectedCalendar> {
    // Non-Linux short-circuit: matches the per-detector pattern
    // used by `scan_chrome_history` and `scan_firefox_history`.
    // We deliberately don't `#[cfg(target_os = "linux")]`-gate
    // the function — keeping it compileable on macOS lets the
    // test suite assert the platform skip on a Linux build host.
    if !matches!(platform, Platform::Linux) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut visited_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
    for root in [
        home.join(".local")
            .join("share")
            .join("evolution")
            .join("calendar"),
        home.join(".config").join("evolution").join("calendar"),
    ] {
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue, // no Evolution store yet — empty result
        };
        for entry in entries.flatten() {
            let source_dir = entry.path();
            if !source_dir.is_dir() {
                continue;
            }
            let source_name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Dedupe: the two roots (`.local/share/...` and
            // `.config/...`) can both point at the same source
            // when Evolution is configured with XDG dirs set to
            // overlap (rare but legal). Walk each source once.
            if !visited_sources.insert(source_name.clone()) {
                continue;
            }
            // Load the per-source email metadata once per source.
            // `load_source_email` returns `None` for missing or
            // malformed `.source` files — we don't fail the scan.
            let email = load_source_email(home, &source_name);
            let profile_label = format_source_profile(&source_name, email.as_deref());

            let cal_entries = match std::fs::read_dir(&source_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for cal in cal_entries.flatten() {
                let ics_path = cal.path();
                if !ics_path.is_file() {
                    continue;
                }
                let ext = ics_path.extension().and_then(|s| s.to_str());
                if ext != Some("ics") {
                    continue;
                }
                // Skip the empty-stub system account. Evolution
                // ships a `local@local-…` stub on fresh installs
                // whose `system-calendar.ics` is zero bytes; we
                // only emit the calendar when the file is
                // non-empty AND carries at least one
                // BEGIN:VCALENDAR marker (so we don't emit a
                // half-written scratch file).
                if is_empty_calendar_stub(&ics_path) {
                    continue;
                }
                let display_name = parse_x_evolution_calendar_name(&ics_path);
                out.push(DetectedCalendar {
                    client: "evolution".to_string(),
                    profile: Some(profile_label.clone()),
                    display_name,
                    ics_path,
                });
            }
        }
    }
    out
}

/// GNOME Calendar (alias) detector. Walks the SAME on-disk roots as
/// [`scan_evolution_calendars`] and emits one `DetectedCalendar`
/// per `.ics` file with `client = "gnome_calendar"`. The detector is
/// gated by two heuristics (both thread-local mockable for tests):
///
/// 1. **GNOME Calendar installed?** (`is_gnome_calendar_installed_for`)
///    — required gate. We only label a calendar as `gnome_calendar`
///    if the `gnome-calendar` binary is on the user's PATH (Linux
///    only; on non-Linux the GUI alias is meaningless — Evolution is
///    not installed there and macOS uses EventKit, not evolution-
///    data-server).
/// 2. **Evolution NOT installed?** (`is_evolution_installed_for`) —
///    when Evolution is installed, [`scan_evolution_calendars`]
///    already emits the same paths with `client = "evolution"`. Emitting
///    again here would create duplicate user-visible rows (the Ask
///    step would render each `.ics` twice, once with each label).
///
/// The two heuristics are independent — the four quadrants map onto:
/// | Evolution | GNOME Calendar | ECD-2 output                         |
/// |-----------|----------------|--------------------------------------|
/// | installed | installed      | empty (ECD-1 emits `evolution`)      |
/// | installed | not installed  | empty (gnome-calendar heuristic)     |
/// | not       | installed      | emit with `client = "gnome_calendar"`|
/// | not       | not installed  | empty (gnome-calendar heuristic)     |
///
/// The ics_path dedup (a defensive `HashSet` over ECD-1's output) is
/// the spec-named control: even if a future heuristic mis-classifies,
/// the dedup catches the path collision. In the four-quadrant matrix
/// above the heuristic + dedup always agree.
///
/// `home` is the user's home dir (the caller resolves `$HOME` or
/// passes a test fixture); `platform` is the runtime-detected
/// [`Platform`] so the function returns empty on non-Linux
/// targets (matching ECD-1's pattern). The function is a pure
/// read-walk — no Mutex held across `.await`, no `OnceLock`.
pub fn scan_gnome_calendar_calendars(home: &Path, platform: &Platform) -> Vec<DetectedCalendar> {
    // Non-Linux short-circuit — matches ECD-1's pattern. The
    // platform check fires BEFORE the heuristic check so a non-Linux
    // host never shells out to `which`.
    if !matches!(platform, Platform::Linux) {
        return Vec::new();
    }
    // GNOME Calendar must actually be installed — otherwise the
    // `gnome_calendar` label is misleading (the user has no GUI to
    // show their calendar). The mock slot is consulted first so
    // tests don't shell out to `which`.
    if !is_gnome_calendar_installed_for(platform) {
        return Vec::new();
    }
    // Evolution takes the labels when it's installed — emit nothing.
    // The user has Evolution → they see Evolution labels (from ECD-1).
    // The user has only GNOME Calendar → we relabel the same .ics
    // files as `gnome_calendar` so the wizard renders the right GUI.
    if is_evolution_installed_for(platform) {
        return Vec::new();
    }
    // Walk the same roots as ECD-1, emitting each `.ics` with the
    // `gnome_calendar` client label. No dedup-vs-ECD-1 here: when
    // only GNOME Calendar is installed, ECD-1's heuristic gate
    // (which lives in ECD-1's detector — same `is_evolution_installed`
    // check) also suppresses its emission, so the on-disk .ics
    // files are surfaced ONCE under the `gnome_calendar` label.
    let mut out = Vec::new();
    let mut visited_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
    for root in [
        home.join(".local")
            .join("share")
            .join("evolution")
            .join("calendar"),
        home.join(".config").join("evolution").join("calendar"),
    ] {
        let entries = match std::fs::read_dir(&root) {
            Ok(e) => e,
            Err(_) => continue, // no Evolution store yet — empty result
        };
        for entry in entries.flatten() {
            let source_dir = entry.path();
            if !source_dir.is_dir() {
                continue;
            }
            let source_name = match entry.file_name().into_string() {
                Ok(s) => s,
                Err(_) => continue,
            };
            // Dedupe across the two roots — same shape as ECD-1's
            // visited_sources walk; documented inline there.
            if !visited_sources.insert(source_name.clone()) {
                continue;
            }
            let email = load_source_email(home, &source_name);
            let profile_label = format_source_profile(&source_name, email.as_deref());

            let cal_entries = match std::fs::read_dir(&source_dir) {
                Ok(e) => e,
                Err(_) => continue,
            };
            for cal in cal_entries.flatten() {
                let ics_path = cal.path();
                if !ics_path.is_file() {
                    continue;
                }
                let ext = ics_path.extension().and_then(|s| s.to_str());
                if ext != Some("ics") {
                    continue;
                }
                if is_empty_calendar_stub(&ics_path) {
                    continue;
                }
                // No path-dedup here: the `is_evolution_installed_for`
                // gate above already excludes the "both installed"
                // case (where ECD-1 emits under `evolution`). When
                // only GNOME Calendar is installed, this loop is the
                // sole emitter — same .ics files, `gnome_calendar`
                // label.
                let display_name = parse_x_evolution_calendar_name(&ics_path);
                out.push(DetectedCalendar {
                    client: "gnome_calendar".to_string(),
                    profile: Some(profile_label.clone()),
                    display_name,
                    ics_path,
                });
            }
        }
    }
    out
}

/// Test seam + production probe for "is the `gnome-calendar` binary
/// on this user's PATH?". In tests the [`GNOME_CALENDAR_PRESENT`]
/// slot is `Some(value)` and we return `value` verbatim — the
/// production probe is skipped. In production the slot is `None`
/// and we shell out to `which gnome-calendar` via
/// `std::process::Command` (no new dep). Non-Linux platforms always
/// return `false`: the GUI alias is meaningless on macOS (which uses
/// EventKit) and Windows (which has no evolution-data-server).
fn is_gnome_calendar_installed_for(platform: &Platform) -> bool {
    if !matches!(platform, Platform::Linux) {
        return false;
    }
    if let Some(mocked) = GNOME_CALENDAR_PRESENT
        .lock()
        .expect("GNOME_CALENDAR_PRESENT mutex poisoned")
        .as_ref()
    {
        return *mocked;
    }
    probe_binary_on_path("gnome-calendar")
}

/// Test seam + production probe for "is the `evolution` binary on
/// this user's PATH?". Same shape as
/// [`is_gnome_calendar_installed_for`]; see that function's docs.
fn is_evolution_installed_for(platform: &Platform) -> bool {
    if !matches!(platform, Platform::Linux) {
        return false;
    }
    if let Some(mocked) = EVOLUTION_PRESENT
        .lock()
        .expect("EVOLUTION_PRESENT mutex poisoned")
        .as_ref()
    {
        return *mocked;
    }
    probe_binary_on_path("evolution")
}

/// Test seam + production probe for "is the `korganizer` binary on
/// this user's PATH?" (Linux only — KOrganizer is a KDE PIM
/// application with no Windows or macOS build). Same shape as
/// [`is_gnome_calendar_installed_for`]; see that function's docs.
/// Non-Linux platforms always return `false`.
fn is_korganizer_installed_for(platform: &Platform) -> bool {
    if !matches!(platform, Platform::Linux) {
        return false;
    }
    if let Some(mocked) = KORGANIZER_PRESENT
        .lock()
        .expect("KORGANIZER_PRESENT mutex poisoned")
        .as_ref()
    {
        return *mocked;
    }
    probe_binary_on_path("korganizer")
}

/// Test seam + production probe for "is Microsoft Outlook installed
/// on this Windows host?" (Windows only — Outlook has no Linux or
/// macOS build of its own). On non-Windows platforms the function
/// always returns `false`.
///
/// Production probe: a `Path::exists` check against the canonical
/// `OUTLOOK.EXE` location under `%ProgramFiles%\Microsoft
/// Office\root\Office16\OUTLOOK.EXE`. We intentionally do NOT
/// reach for the `winreg` crate (the proposal's "use winreg only
/// if it's already a transitive dep" guidance) — checking the
/// binary's presence on disk is sufficient evidence for the
/// UX-fallback hint and avoids a new dependency. The
/// `%ProgramFiles%` env-var resolution is done via
/// `std::env::var("ProgramFiles")` so the test seam can override
/// it via the standard env-mutation pattern if needed; in
/// practice the unit tests consult the `OUTLOOK_PRESENT` mock
/// instead so the env-var is never read in test mode.
fn is_outlook_installed_for(platform: &Platform) -> bool {
    // The Windows build is the only host where Outlook ships;
    // short-circuit everything else (including `Platform::Linux`,
    // `Platform::Macos`, and the catch-all `Platform::Other("...")`
    // for non-Windows OSes like "freebsd" or "linux"). The check
    // matches the spec's "non-Linux / non-Windows platforms return
    // Unavailable" — and also covers the `Platform::Other("linux")`
    // case that surfaces on a Windows binary built and run on a
    // Linux test host (which is what the unit tests do).
    let is_windows = match platform {
        Platform::Other(os) if os.eq_ignore_ascii_case("windows") => true,
        _ => false,
    };
    if !is_windows {
        return false;
    }
    if let Some(mocked) = OUTLOOK_PRESENT
        .lock()
        .expect("OUTLOOK_PRESENT mutex poisoned")
        .as_ref()
    {
        return *mocked;
    }
    probe_outlook_exe()
}

/// Production probe for Microsoft Outlook. Resolves
/// `%ProgramFiles%\Microsoft Office\root\Office16\OUTLOOK.EXE`
/// and returns `true` iff that path exists. We don't try to
/// handle every Office install variant (32-bit, 2019, O365
/// per-machine, etc.) — the canonical Office16 path covers
/// the Office 2016+ default install and is the right answer
/// for the "is Outlook here?" question the UX-fallback hint
/// needs to ask.
///
/// The function is only ever called from
/// `is_outlook_installed_for` on a Windows target, but we
/// don't `#[cfg(target_os = "windows")]`-gate it so the
/// function exists on all build targets (it just returns
/// `false` on non-Windows — the env-var and path-existence
/// checks both work cross-platform, and a Linux build host
/// running unit tests never reaches this code path because
/// `is_outlook_installed_for` short-circuits on
/// non-`Platform::Other("windows")` platforms).
fn probe_outlook_exe() -> bool {
    let program_files = match std::env::var("ProgramFiles") {
        Ok(p) if !p.is_empty() => p,
        // `ProgramFiles(x86)` is the 32-bit-on-64-bit-Windows
        // install location; we fall back to it for completeness
        // (a 32-bit Office install lands there). The 64-bit
        // install is the common case so the primary `ProgramFiles`
        // lookup runs first.
        _ => match std::env::var("ProgramFiles(x86)") {
            Ok(p) if !p.is_empty() => p,
            _ => return false,
        },
    };
    let candidate = Path::new(&program_files)
        .join("Microsoft Office")
        .join("root")
        .join("Office16")
        .join("OUTLOOK.EXE");
    candidate.is_file()
}

/// Run `which <binary>` (POSIX) via `std::process::Command`. Returns
/// `true` iff the binary is on PATH. We don't care about the path
/// `which` prints — just the exit code. We use `Command::new("which")`
/// rather than the `which` crate to honor the no-new-dep rule (the
/// existing codebase does the same for `gh auth status` at
/// `probe_gh_auth`).
fn probe_binary_on_path(binary: &str) -> bool {
    std::process::Command::new("which")
        .arg(binary)
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Read `<home>/.config/evolution/sources/<source_name>.source`
/// and try to pull an email out of it. Evolution's `.source`
/// files are JSON-ish (key-value lines, not strict JSON); the
/// only field we care about is `parent[1].text` which carries
/// the user's email for CalDAV/IMAP sources. Returns `None` on
/// any parse failure — the scanner is best-effort.
fn load_source_email(home: &Path, source_name: &str) -> Option<String> {
    let path = home
        .join(".config")
        .join("evolution")
        .join("sources")
        .join(format!("{source_name}.source"));
    let raw = std::fs::read_to_string(&path).ok()?;
    for line in raw.lines() {
        // The format Evolution actually uses looks like:
        // `[parent[1].text]` / `... value: ...` then the value
        // on subsequent lines. We grep for the marker line and
        // return the trimmed next non-empty line.
        if line.trim_start().starts_with("[parent[1].text]") {
            // The actual value sits on the following line(s).
            // Walk forward to the first non-empty line.
            for next in raw.lines().skip_while(|l| l.trim().is_empty()) {
                let trimmed = next.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('[') {
                    return Some(trimmed.to_string());
                }
            }
        }
    }
    None
}

/// Compose the `profile` label shown to the user. The base label
/// is the source directory name (e.g. `"local@local-…"`,
/// `"Google"`, `"CalDAV — work"`); when an email is available
/// we append it in parens so the user can disambiguate
/// multiple CalDAV sources.
fn format_source_profile(source_name: &str, email: Option<&str>) -> String {
    match email {
        Some(addr) if !addr.is_empty() => format!("{source_name} ({addr})"),
        _ => source_name.to_string(),
    }
}

/// A "stub" Evolution system calendar: the file exists, is a
/// regular file, but is zero bytes (or carries only an empty
/// `BEGIN:VCALENDAR` / `END:VCALENDAR` envelope with no events).
/// Evolution creates the stub on first launch so the system
/// source has *something* to point at; we don't want to surface
/// it as a user-meaningful calendar.
fn is_empty_calendar_stub(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return true;
    };
    if meta.len() == 0 {
        return true;
    }
    // Cheap heuristic: a stub has BEGIN:VCALENDAR but no
    // BEGIN:VEVENT. We do a single read (the file is small;
    // Evolution calendar files are <1 MB even with years of
    // events) and look for the marker.
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    raw.contains("BEGIN:VCALENDAR") && !raw.contains("BEGIN:VEVENT")
}

/// Extract the `X-EVOLUTION-CALENDAR` property value from the
/// `.ics` file. Returns `None` when the property is absent or
/// the file can't be read. We only need the first occurrence —
/// the property is meant to appear once per calendar.
fn parse_x_evolution_calendar_name(path: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(path).ok()?;
    let mut current = String::new();
    let mut first_line = true;
    for line in raw.lines() {
        // RFC 5545 §3.1: a line starting with a space or tab
        // is a continuation of the previous logical line. We
        // un-fold by accumulating into `current`.
        if line.starts_with(' ') || line.starts_with('\t') {
            current.push_str(line.trim_start());
            continue;
        }
        // Non-continuation line: the previous logical line (in
        // `current`) just ended. If it was `X-EVOLUTION-CALENDAR`,
        // return its value. Skip the check on the very first
        // iteration when `current` is empty.
        if !first_line {
            if let Some(rest) = current.strip_prefix("X-EVOLUTION-CALENDAR") {
                if let Some(v) = rest.strip_prefix(':') {
                    return Some(v.trim().to_string());
                }
            }
        }
        current = line.to_string();
        first_line = false;
    }
    // Flush the trailing logical line (no terminator newline).
    if !first_line {
        if let Some(rest) = current.strip_prefix("X-EVOLUTION-CALENDAR") {
            if let Some(v) = rest.strip_prefix(':') {
                return Some(v.trim().to_string());
            }
        }
    }
    None
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
                                EvidenceKind::FileExists {
                                    path: PathBuf::new(),
                                },
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
            // ECD-4 — KOrganizer UX-fallback probe. Even when no
            // Evolution store exists, a Linux host may have
            // KOrganizer installed (KDE PIM). We surface its
            // presence via the calendar `notes` so the wizard can
            // show the "KOrganizer detected; export via File →
            // Export → iCalendar" hint. The probe lives in
            // `scan_korganizer` (a `CollectorCandidate`-returning
            // function) so the unit tests can exercise the
            // heuristic mock independently of the orchestrator.
            let korganizer = scan_korganizer(platform);
            let korganizer_note = korganizer_notes_fragment(&korganizer);
            let candidates = [
                home.join(".config").join("evolution"),
                home.join(".local").join("share").join("evolution"),
            ];
            for dir in &candidates {
                if let Some(ev) = probe_dir(dir) {
                    // Probe the per-source `.ics` files (the
                    // detector introduced for ECD-1 — the
                    // evolution-calendar auto-discover). The
                    // count goes into `notes` so the wizard
                    // shows "evolution: N calendars" without
                    // having to invoke a second command. The
                    // actual `Vec<DetectedCalendar>` is
                    // surfaced by `scan_evolution_calendars`
                    // directly (the LLM step consumes that).
                    //
                    // ECD-2 — also probe the GNOME Calendar
                    // alias detector. When the user has GNOME
                    // Calendar installed (and Evolution is NOT),
                    // the GNOME Calendar detector emits with a
                    // distinct `client = "gnome_calendar"` label.
                    // When both are installed, ECD-2 emits
                    // nothing (the heuristic + ics_path dedup
                    // keep ECD-1's `evolution` label as the
                    // user-facing row). The combined vector is
                    // what the LLM step consumes.
                    //
                    // ECD-4 — also append the KOrganizer
                    // UX-fallback fragment when KOrganizer is
                    // installed. The fragment is "; KOrganizer
                    // detected; please export your calendar via
                    // File → Export → iCalendar" so the user
                    // sees the hint in the same notes string
                    // the wizard already renders.
                    // ECD-3 — also probe the Thunderbird cross-OS
                    // detector. The probe runs unconditionally here
                    // (any platform with `~/.thunderbird/` or its
                    // platform-equivalent will surface calendars);
                    // the detector itself is platform-short-
                    // circuited for non-Linux/Windows/macOS
                    // hosts (see `scan_thunderbird_calendars`).
                    //
                    // ECD-4 — also append the KOrganizer
                    // UX-fallback fragment when KOrganizer is
                    // installed. The fragment is "; KOrganizer
                    // detected; please export your calendar via
                    // File → Export → iCalendar" so the user
                    // sees the hint in the same notes string
                    // the wizard already renders.
                    let detected = scan_evolution_calendars(home, platform);
                    let detected_gnome = scan_gnome_calendar_calendars(home, platform);
                    let detected_thunderbird = scan_thunderbird_calendars(home, platform);
                    let count = detected.len();
                    let gnome_count = detected_gnome.len();
                    let thunderbird_count = detected_thunderbird.len();
                    let base_notes = match (count > 0, gnome_count > 0, thunderbird_count > 0) {
                        (true, true, true) => Some(format!(
                            "evolution + GNOME Calendar + Thunderbird stores present; \
                             auto-discovered {count} evolution {}, \
                             {gnome_count} GNOME Calendar {}, \
                             and {thunderbird_count} Thunderbird {}",
                            if count == 1 { "calendar" } else { "calendars" },
                            if gnome_count == 1 { "alias" } else { "aliases" },
                            if thunderbird_count == 1 {
                                "calendar"
                            } else {
                                "calendars"
                            },
                        )),
                        (true, false, true) => Some(format!(
                            "evolution + Thunderbird stores present; \
                             auto-discovered {count} evolution {} and \
                             {thunderbird_count} Thunderbird {}",
                            if count == 1 { "calendar" } else { "calendars" },
                            if thunderbird_count == 1 {
                                "calendar"
                            } else {
                                "calendars"
                            },
                        )),
                        (false, true, true) => Some(format!(
                            "GNOME Calendar + Thunderbird detected; \
                             auto-discovered {gnome_count} GNOME Calendar {} and \
                             {thunderbird_count} Thunderbird {}",
                            if gnome_count == 1 { "alias" } else { "aliases" },
                            if thunderbird_count == 1 {
                                "calendar"
                            } else {
                                "calendars"
                            },
                        )),
                        (true, true, false) => Some(format!(
                            "evolution calendar store present; \
                             auto-discovered {count} {} + \
                             {gnome_count} GNOME Calendar {}",
                            if count == 1 { "calendar" } else { "calendars" },
                            if gnome_count == 1 { "alias" } else { "aliases" },
                        )),
                        (true, false, false) => Some(format!(
                            "evolution calendar store present; \
                             auto-discovered {count} {}",
                            if count == 1 { "calendar" } else { "calendars" }
                        )),
                        (false, true, false) => Some(format!(
                            "GNOME Calendar detected; \
                             auto-discovered {gnome_count} {}",
                            if gnome_count == 1 {
                                "calendar"
                            } else {
                                "calendars"
                            },
                        )),
                        (false, false, true) => Some(format!(
                            "Thunderbird detected; \
                             auto-discovered {thunderbird_count} {}",
                            if thunderbird_count == 1 {
                                "calendar"
                            } else {
                                "calendars"
                            },
                        )),
                        (false, false, false) => None,
                    };
                    let notes = merge_notes(base_notes, korganizer_note.as_deref());
                    return finalize(
                        "calendar",
                        "Calendar events",
                        CollectorStatus::Available,
                        ev,
                        notes,
                    );
                }
            }
            // Evolution store absent — but KOrganizer may still
            // be installed (KDE-only host with no Evolution at
            // all). Surface it via `notes` so the wizard shows
            // the export hint even on a KOrganizer-only host.
            // When KOrganizer is also absent we return the
            // original "no evolution calendar store found"
            // note.
            finalize(
                "calendar",
                "Calendar events",
                CollectorStatus::Unavailable,
                unavailable_evidence(),
                merge_notes(
                    Some("no evolution calendar store found".to_string()),
                    korganizer_note.as_deref(),
                ),
            )
        }
        Platform::Other(os) => {
            // ECD-4 — Windows branch for Outlook UX-fallback.
            // `Platform::Other("windows")` is how `detect_platform`
            // reports the host on Windows builds; the
            // `is_outlook_installed_for` probe short-circuits to
            // `false` on every other `Platform::Other("...")`
            // variant, so non-Windows platforms (freebsd, etc.)
            // still see the "not yet supported" note. When
            // Outlook IS installed, we upgrade the note to the
            // "Outlook detected; per-calendar .ics export" hint
            // so the wizard shows the right UX-fallback text.
            let is_windows = os.eq_ignore_ascii_case("windows");
            if is_windows {
                let outlook = scan_outlook(platform);
                if outlook.status == CollectorStatus::Available {
                    return outlook;
                }
                return finalize(
                    "calendar",
                    "Calendar events",
                    CollectorStatus::Unavailable,
                    unavailable_evidence(),
                    Some(format!("calendar collector not yet supported on {os}")),
                );
            }
            finalize(
                "calendar",
                "Calendar events",
                CollectorStatus::Unavailable,
                unavailable_evidence(),
                Some(format!("calendar collector not yet supported on {os}")),
            )
        }
    }
}

/// ECD-4 — KOrganizer UX-fallback detector. Linux-only. When the
/// `korganizer` binary is on the user's PATH we report
/// `Available` with `CommandExists` evidence + a UX-fallback note
/// asking the user to export their calendar via File → Export →
/// iCalendar. On non-Linux platforms we report `Unavailable` with
/// a platform-skip note.
///
/// The function is intentionally separate from the calendar
/// orchestrator (`scan_calendar`) so the unit tests can exercise
/// the heuristic mock independently of the Evolution/GNOME
/// Calendar plumbing. The orchestrator consumes this function's
/// result to render the per-OS notes string; the wizard reads
/// that string verbatim to decide whether to show the export
/// hint.
pub fn scan_korganizer(platform: &Platform) -> CollectorCandidate {
    if !matches!(platform, Platform::Linux) {
        return finalize(
            "korganizer",
            "KOrganizer (Linux)",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("KOrganizer is Linux-only".to_string()),
        );
    }
    if is_korganizer_installed_for(platform) {
        // The exact path doesn't matter for the UX-fallback
        // hint (the user is asked to export, not to point us
        // at a pre-existing file). We still surface a
        // placeholder binary path on the evidence record so
        // the JSON is consistent with the other CommandExists
        // rows (`which` returns the resolved absolute path; we
        // just want a non-empty placeholder here that the
        // test can pin if needed). Using `/usr/bin/korganizer`
        // as the canonical guess is good enough — the
        // function-level guarantee is "korganizer is
        // installed", not "korganizer is at this exact path".
        return finalize(
            "korganizer",
            "KOrganizer (Linux)",
            CollectorStatus::Available,
            EvidenceKind::CommandExists {
                binary: "korganizer".to_string(),
                path: PathBuf::from("/usr/bin/korganizer"),
            },
            Some(
                "KOrganizer is installed; please export your calendar via \
                 File → Export → iCalendar and paste the path below"
                    .to_string(),
            ),
        );
    }
    finalize(
        "korganizer",
        "KOrganizer (Linux)",
        CollectorStatus::Unavailable,
        unavailable_evidence(),
        Some("korganizer not on PATH".to_string()),
    )
}

/// ECD-4 — Outlook UX-fallback detector. Windows-only. When
/// `%ProgramFiles%\Microsoft Office\root\Office16\OUTLOOK.EXE`
/// exists we report `Available` + the per-calendar `.ics` export
/// hint. On non-Windows platforms we report `Unavailable` with
/// the platform-skip note.
pub fn scan_outlook(platform: &Platform) -> CollectorCandidate {
    let is_windows = match platform {
        Platform::Other(os) if os.eq_ignore_ascii_case("windows") => true,
        _ => false,
    };
    if !is_windows {
        return finalize(
            "outlook",
            "Outlook (Windows)",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("Outlook is Windows-only".to_string()),
        );
    }
    if is_outlook_installed_for(platform) {
        return finalize(
            "outlook",
            "Outlook (Windows)",
            CollectorStatus::Available,
            EvidenceKind::FileExists {
                path: PathBuf::from(
                    "C:\\Program Files\\Microsoft Office\\root\\Office16\\OUTLOOK.EXE",
                ),
            },
            Some(
                "Outlook is installed; the calendar collector will read via \
                 per-calendar .ics export. Use Outlook File → Save As → \
                 iCalendar Format for each calendar you want to include."
                    .to_string(),
            ),
        );
    }
    finalize(
        "outlook",
        "Outlook (Windows)",
        CollectorStatus::Unavailable,
        unavailable_evidence(),
        Some("OUTLOOK.EXE not found in Microsoft Office 16 install".to_string()),
    )
}

/// Build the "; KOrganizer detected; please export …" notes
/// fragment when KOrganizer is installed, `None` otherwise.
/// Centralised so the orchestrator (`scan_calendar`) and the
/// standalone test surface stay in sync — a regression that
/// changes the hint text in one place would be caught by the
/// value-asserting test in the other.
fn korganizer_notes_fragment(korganizer: &CollectorCandidate) -> Option<String> {
    if korganizer.status == CollectorStatus::Available {
        // The hint text is identical to `scan_korganizer`'s
        // `notes` string when the probe fires; we just prefix
        // it with "; " so the orchestrator can append it
        // cleanly after the existing evolution/gnome
        // fragment.
        Some(format!(
            "; {}",
            korganizer
                .notes
                .as_deref()
                .unwrap_or("KOrganizer is installed")
        ))
    } else {
        None
    }
}

/// Append `extra` to `base` with a "; " separator. Both inputs
/// may be `None` — the result is the `Option<String>` value
/// that's actually present (or `None` when both are absent).
/// Used by [`scan_calendar`] to merge the evolution/gnome base
/// notes with the KOrganizer UX-fallback fragment.
fn merge_notes(base: Option<String>, extra: Option<&str>) -> Option<String> {
    match (base, extra) {
        (Some(b), Some(e)) => Some(format!("{b}{e}")),
        (Some(b), None) => Some(b),
        (None, Some(e)) => Some(e.to_string()),
        (None, None) => None,
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
            // `Authorized` is deprecated and an alias for
            // `FullAccess` (`Self = Self(FullAccess.0)` per
            // objc2-event-kit 0.3.2's EKTypes.rs), so we don't
            // need a separate arm — `FullAccess` covers both.
            EKAuthorizationStatus::FullAccess => Some(CalendarEventKitTcc::FullAccess),
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

/// Firefox history lives at a per-profile path. The profile dir
/// is randomly named (`xxxxxxxx.default-release` or
/// `xxxxxxxx.default`), so we glob for `places.sqlite` under the
/// Firefox profiles root. The probe accepts the FIRST match
/// (Firefox keeps the active profile at the top in `profiles.ini`,
/// but the dir layout is stable enough that the first match is
/// usually the right one).
///
/// macOS: `~/Library/Application Support/Firefox/Profiles/<profile>/places.sqlite`
/// Linux: `~/.mozilla/firefox/<profile>/places.sqlite`
///
/// Notes for the future collector (not built in this PR — this
/// commit is scanner-only, same shape as the Chrome/Brave probes):
/// - Firefox stores bookmarks, history, and form data in the same
///   `places.sqlite`. The history rows live in `moz_places`.
/// - The database is locked when Firefox is running. Use a copy +
///   read pattern, or `sqlite3_open_v2(...SQLITE_OPEN_READONLY)`.
///   PRAGMA `journal_mode = WAL` survives a copy.
fn scan_firefox_history(home: &Path, platform: &Platform) -> CollectorCandidate {
    let profiles_root = match platform {
        Platform::Macos => home
            .join("Library")
            .join("Application Support")
            .join("Firefox")
            .join("Profiles"),
        Platform::Linux => home.join(".mozilla").join("firefox"),
        Platform::Other(_) => PathBuf::new(),
    };
    if profiles_root.as_os_str().is_empty() {
        return finalize(
            "firefox_history",
            "Firefox history",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("platform not supported".to_string()),
        );
    }
    // Glob `places.sqlite` under each `<profile>/` subdirectory.
    // `glob` returns absolute paths when given an absolute root.
    let path = glob_first_child_with(&profiles_root, "places.sqlite");
    if let Some(p) = path {
        return finalize(
            "firefox_history",
            "Firefox history",
            CollectorStatus::Available,
            EvidenceKind::FileExists { path: p },
            None,
        );
    }
    finalize(
        "firefox_history",
        "Firefox history",
        CollectorStatus::Unavailable,
        unavailable_evidence(),
        Some(format!("{} not present", profiles_root.display())),
    )
}

/// Opera history: Chromium-based, so the path layout is the same
/// as Chrome but with a different vendor dir. Opera also keeps a
/// `Local State` JSON at the vendor root that names the active
/// profile — but for the scanner we just probe the default
/// `History` file (the user with custom profiles can re-run the
/// scan and the collector's profile-detection logic will pick the
/// right one once that's built).
///
/// macOS: `~/Library/Application Support/com.operasoftware.Opera/History`
/// Linux: `~/.config/opera/Default/History`
fn scan_opera_history(home: &Path, platform: &Platform) -> CollectorCandidate {
    let path = match platform {
        Platform::Macos => home
            .join("Library")
            .join("Application Support")
            .join("com.operasoftware.Opera")
            .join("History"),
        Platform::Linux => home
            .join(".config")
            .join("opera")
            .join("Default")
            .join("History"),
        Platform::Other(_) => PathBuf::new(),
    };
    if path.as_os_str().is_empty() {
        return finalize(
            "opera_history",
            "Opera history",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("platform not supported".to_string()),
        );
    }
    if let Some(ev) = probe_file(&path) {
        return finalize(
            "opera_history",
            "Opera history",
            CollectorStatus::Available,
            ev,
            None,
        );
    }
    finalize(
        "opera_history",
        "Opera history",
        CollectorStatus::Unavailable,
        unavailable_evidence(),
        Some(format!("{} not present", path.display())),
    )
}

/// Safari history (macOS only — Safari doesn't exist on Linux).
/// `~/Library/Safari/History.db` is the SQLite database. The
/// adjacent `HistoryIndex.plist` is a Spotlight-style index that
/// Safari writes alongside it; either one being present is
/// sufficient evidence the user uses Safari (the actual collector
/// will read `History.db`).
///
/// Full Disk Access: Safari's `History.db` is gated by TCC. The
/// scanner reports file existence (the OS reports the path even
/// without FDA), but the collector that reads it later will need
/// FDA granted to Trail — same shape as the EventKit TCC probe.
fn scan_safari_history(home: &Path, platform: &Platform) -> CollectorCandidate {
    let path = match platform {
        Platform::Macos => home.join("Library").join("Safari").join("History.db"),
        // Safari doesn't ship on Linux — keep the candidate in the
        // report with `Unavailable` + a platform-unavailable note
        // so the wizard UI doesn't show a phantom row.
        Platform::Linux | Platform::Other(_) => PathBuf::new(),
    };
    if path.as_os_str().is_empty() {
        return finalize(
            "safari_history",
            "Safari history",
            CollectorStatus::Unavailable,
            unavailable_evidence(),
            Some("platform not supported".to_string()),
        );
    }
    if let Some(ev) = probe_file(&path) {
        return finalize(
            "safari_history",
            "Safari history",
            CollectorStatus::Available,
            ev,
            None,
        );
    }
    finalize(
        "safari_history",
        "Safari history",
        CollectorStatus::Unavailable,
        unavailable_evidence(),
        Some(format!("{} not present", path.display())),
    )
}

/// Walk one level deep under `root`, return the first directory
/// that contains a file named `filename`. Used by the Firefox
/// probe to skip the random `<profile>` directory name. Returns
/// `None` if `root` doesn't exist or has no qualifying child.
///
/// We don't pull in the `glob` crate to avoid a new dep — the
/// probe scans `read_dir` once per browser and stops at the first
/// hit. Firefox users typically have a single active profile.
fn glob_first_child_with(root: &Path, filename: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let candidate = path.join(filename);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
            "firefox_history",
            "opera_history",
            "safari_history",
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
    fn firefox_history_finds_places_sqlite_in_profile_dir() {
        // The Firefox profile dir is randomly named (e.g.
        // `xxxxxxxx.default-release`). The probe should walk
        // one level deep under `Profiles/` and find the file
        // regardless of the random prefix.
        let home = TempHome::new();
        let platform = detect_platform();
        let profile_dir = match platform {
            Platform::Macos => home
                .path()
                .join("Library/Application Support/Firefox/Profiles/abcd1234.default-release"),
            Platform::Linux => home
                .path()
                .join(".mozilla/firefox/abcd1234.default-release"),
            Platform::Other(_) => return, // skip on Other platforms
        };
        std::fs::create_dir_all(&profile_dir).unwrap();
        // Non-empty: `probe_file` requires `meta.len() != 0`
        // (a zero-byte lock-stub doesn't count as evidence).
        std::fs::write(profile_dir.join("places.sqlite"), b"x").unwrap();

        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let c = find(&report, "firefox_history");
        assert_eq!(c.status, CollectorStatus::Available);
        match &c.evidence {
            EvidenceKind::FileExists { path } => {
                assert!(path.ends_with("places.sqlite"));
                assert!(path.to_string_lossy().contains("abcd1234.default-release"));
            }
            _ => panic!("expected FileExists for firefox history"),
        }
    }

    #[test]
    fn firefox_history_unavailable_without_profiles_dir() {
        let home = TempHome::new();
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let c = find(&report, "firefox_history");
        assert_eq!(c.status, CollectorStatus::Unavailable);
    }

    #[test]
    fn opera_history_evidence_path_correct_per_platform() {
        let home = TempHome::new();
        let platform = detect_platform();
        let path = match platform {
            // macOS has no `.config` dir; Opera lands under
            // `Library/Application Support/com.operasoftware.Opera`.
            Platform::Macos => {
                let p = home
                    .path()
                    .join("Library/Application Support/com.operasoftware.Opera/History");
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                // Non-empty: `probe_file` requires `meta.len() != 0`.
                std::fs::write(&p, b"x").unwrap();
                p
            }
            Platform::Linux => {
                let p = home.path().join(".config/opera/Default/History");
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(&p, b"x").unwrap();
                p
            }
            Platform::Other(_) => return,
        };
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let c = find(&report, "opera_history");
        assert_eq!(c.status, CollectorStatus::Available);
        match &c.evidence {
            EvidenceKind::FileExists { path: found } => assert_eq!(found, &path),
            _ => panic!("expected FileExists for opera history"),
        }
    }

    #[test]
    fn safari_history_only_reported_on_macos() {
        // On Linux the candidate is present in the report with
        // `Unavailable` (no phantom row) so the wizard UI
        // doesn't show Safari as a Linux option.
        let home = TempHome::new();
        let platform = detect_platform();
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let c = find(&report, "safari_history");
        // Status may be Available or Unavailable depending on
        // host, but on Linux it must NEVER be Available.
        if matches!(platform, Platform::Linux | Platform::Other(_)) {
            assert_eq!(c.status, CollectorStatus::Unavailable);
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

    // -----------------------------------------------------------------------
    // ECD-1 — Evolution calendar auto-discover detector
    // -----------------------------------------------------------------------

    /// Helper to write a non-stub `.ics` file (one with
    /// BEGIN:VEVENT, so the stub-skip heuristic doesn't reject it).
    /// `body` carries the full file contents.
    fn write_ics(home: &TempHome, rel: &str, body: &str) -> PathBuf {
        let p = home.path().join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&p, body).unwrap();
        p
    }

    /// Minimal iCalendar body carrying an `X-EVOLUTION-CALENDAR`
    /// property and one VEVENT so the detector accepts it as
    /// non-empty.
    fn evolution_ics(display_name: &str) -> String {
        format!(
            "BEGIN:VCALENDAR\r\n\
             VERSION:2.0\r\n\
             PRODID:-//GNOME//Evolution {display_name}//EN\r\n\
             X-EVOLUTION-CALENDAR:{display_name}\r\n\
             BEGIN:VEVENT\r\n\
             UID:stub@example.com\r\n\
             SUMMARY:hello\r\n\
             END:VEVENT\r\n\
             END:VCALENDAR\r\n"
        )
    }

    /// `scan_evolution_calendars` on a fixture dir returns one
    /// `DetectedCalendar` per non-stub `.ics` file, with the source
    /// dir name carried in `profile` and the `X-EVOLUTION-CALENDAR`
    /// value carried in `display_name`.
    #[test]
    fn scan_evolution_calendars_finds_ics_per_source() {
        let home = TempHome::new();
        // Two sources, three calendars total: source `1234567890`
        // has 2 calendars (one with a display name, one without);
        // source `google-9876` has 1 calendar with a display name.
        // The system stub (`local@local-0`) is written but must
        // be skipped.
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/work-calendar.ics",
            &evolution_ics("Work Calendar"),
        );
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/personal.ics",
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:p@example.com\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n",
        );
        write_ics(
            &home,
            ".local/share/evolution/calendar/google-9876/birthdays.ics",
            &evolution_ics("Birthdays"),
        );
        write_ics(
            &home,
            ".local/share/evolution/calendar/local@local-0/system-calendar.ics",
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
        );

        let detected = scan_evolution_calendars(home.path(), &Platform::Linux);
        // Expect 3 calendars: 2 from `1234567890`, 1 from
        // `google-9876`. The `local@local-0/system-calendar.ics`
        // stub has BEGIN:VCALENDAR but no BEGIN:VEVENT → skipped.
        assert_eq!(
            detected.len(),
            3,
            "expected 3 detected calendars, got {detected:?}"
        );
        for cal in &detected {
            assert_eq!(cal.client, "evolution");
            assert!(cal.ics_path.extension().and_then(|s| s.to_str()) == Some("ics"));
            assert!(cal.profile.is_some());
        }
        // Verify the display names came back where expected.
        let work_cal = detected
            .iter()
            .find(|c| c.display_name.as_deref() == Some("Work Calendar"))
            .expect("Work Calendar must be present");
        assert!(work_cal.ics_path.ends_with("work-calendar.ics"));
        let birthdays = detected
            .iter()
            .find(|c| c.display_name.as_deref() == Some("Birthdays"))
            .expect("Birthdays must be present");
        assert!(birthdays.ics_path.ends_with("google-9876/birthdays.ics"));
        // The unnamed calendar (no X-EVOLUTION-CALENDAR property)
        // comes back with `display_name: None`.
        let unnamed = detected
            .iter()
            .find(|c| c.display_name.is_none())
            .expect("unnamed calendar must be present");
        assert!(unnamed.ics_path.ends_with("personal.ics"));
    }

    /// The per-source email extracted from
    /// `~/.config/evolution/sources/*.source` (the `parent[1].text`
    /// field) is appended to the profile label.
    #[test]
    fn scan_evolution_calendars_picks_up_source_email() {
        let home = TempHome::new();
        write_ics(
            &home,
            ".local/share/evolution/calendar/9876543210/work.ics",
            &evolution_ics("Work"),
        );
        // Write a `.source` file in Evolution's key=value line
        // format (not strict JSON, but with the `[parent[1].text]`
        // header the detector looks for).
        std::fs::create_dir_all(home.path().join(".config/evolution/sources")).unwrap();
        std::fs::write(
            home.path()
                .join(".config/evolution/sources/9876543210.source"),
            "[Calendar]\n[parent[1].text]\nwork@example.com\n",
        )
        .unwrap();

        let detected = scan_evolution_calendars(home.path(), &Platform::Linux);
        assert_eq!(detected.len(), 1);
        let cal = &detected[0];
        let profile = cal.profile.as_deref().expect("profile must be populated");
        assert!(
            profile.contains("work@example.com"),
            "profile must carry the source email; got: {profile}"
        );
        assert!(
            profile.contains("9876543210"),
            "profile must carry the source directory name; got: {profile}"
        );
    }

    /// Non-Linux platforms return empty — the detector is
    /// Evolution-specific and the test seam matches the existing
    /// per-detector pattern.
    #[test]
    fn scan_evolution_calendars_empty_on_non_linux() {
        let home = TempHome::new();
        // Stage the fixture so a Linux run would find at least
        // one calendar — that way the "empty on non-Linux"
        // assertion can't be masked by a missing fixture.
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234/cal.ics",
            &evolution_ics("Cal"),
        );
        for platform in [
            Platform::Macos,
            Platform::Other("windows".to_string()),
            Platform::Other("freebsd".to_string()),
        ] {
            let detected = scan_evolution_calendars(home.path(), &platform);
            assert!(
                detected.is_empty(),
                "non-Linux platform {platform:?} must return empty; got {detected:?}"
            );
        }
    }

    /// The system account (`local@*/`) stub — the empty
    /// `BEGIN:VCALENDAR` envelope Evolution writes on a fresh
    /// install — must be skipped.
    #[test]
    fn scan_evolution_calendars_skips_system_account_stub() {
        let home = TempHome::new();
        // Stub: BEGIN:VCALENDAR but no VEVENT.
        write_ics(
            &home,
            ".local/share/evolution/calendar/local@local-0/system-calendar.ics",
            "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nEND:VCALENDAR\r\n",
        );
        // Real calendar in a non-system source — must be emitted.
        write_ics(
            &home,
            ".local/share/evolution/calendar/abc/work.ics",
            &evolution_ics("Work"),
        );
        let detected = scan_evolution_calendars(home.path(), &Platform::Linux);
        assert_eq!(
            detected.len(),
            1,
            "stub must be skipped, real calendar must be kept; got {detected:?}"
        );
        assert!(detected[0].ics_path.ends_with("work.ics"));
    }

    /// The orchestrator wires the detector's count into the
    /// `calendar` candidate's `notes` so the wizard UI shows
    /// "auto-discovered N calendars" without a second IPC call.
    #[test]
    fn scan_calendar_linux_notes_carry_evolution_count() {
        let home = TempHome::new();
        write_ics(
            &home,
            ".local/share/evolution/calendar/abc/work.ics",
            &evolution_ics("Work"),
        );
        write_ics(
            &home,
            ".local/share/evolution/calendar/abc/personal.ics",
            &evolution_ics("Personal"),
        );
        let platform = Platform::Linux;
        let report = scan_laptop_with_config(
            &platform,
            home.path(),
            &home.path().join(".trail/config.json"),
        );
        let c = find(&report, "calendar");
        assert_eq!(c.status, CollectorStatus::Available);
        let notes = c.notes.as_deref().unwrap_or("");
        assert!(
            notes.contains("auto-discovered 2 calendars"),
            "notes must report the discovered count; got: {notes}"
        );
    }

    // -----------------------------------------------------------------------
    // ECD-2 — GNOME Calendar alias detector
    //
    // GNOME Calendar piggybacks on evolution-data-server, so the on-disk
    // `.ics` files are identical to ECD-1's walk. We treat GNOME Calendar
    // as a SEPARATE `client` label so the Ask step renders "GNOME Calendar"
    // for users who install only that GUI (no Evolution MUA).
    //
    // The detector has two heuristics, both thread-local mockable for
    // tests:
    //   - `is_gnome_calendar_installed_for(platform)` — required gate.
    //   - `is_evolution_installed_for(platform)` — gate that suppresses
    //     emission when Evolution is installed (ECD-1 emits the entries
    //     with `client = "evolution"` in that case; emitting here would
    //     duplicate the user-visible row).
    //
    // The ics_path dedup in the implementation (build a HashSet from
    // ECD-1's output) is a defensive layer; the heuristic gates are the
    // authoritative control.
    // -----------------------------------------------------------------------

    /// Run `body` with both heuristic mocks set to `gc_installed` and
    /// `evolution_installed`. The guard restores the previous values on
    /// drop (panic-safe), so subsequent tests start from the
    /// "no mock" default.
    fn with_heuristics<F>(gc_installed: bool, evolution_installed: bool, body: F)
    where
        F: FnOnce(),
    {
        let prev_gc = GNOME_CALENDAR_PRESENT
            .lock()
            .expect("heuristic mutex")
            .replace(gc_installed);
        let prev_ev = EVOLUTION_PRESENT
            .lock()
            .expect("heuristic mutex")
            .replace(evolution_installed);
        let _restore = HeuristicsGuard { prev_gc, prev_ev };
        body();
    }

    /// RAII guard that restores both heuristic mocks on drop.
    struct HeuristicsGuard {
        prev_gc: Option<bool>,
        prev_ev: Option<bool>,
    }

    impl Drop for HeuristicsGuard {
        fn drop(&mut self) {
            let mut gc = GNOME_CALENDAR_PRESENT.lock().expect("heuristic mutex");
            *gc = self.prev_gc;
            let mut ev = EVOLUTION_PRESENT.lock().expect("heuristic mutex");
            *ev = self.prev_ev;
        }
    }

    /// Value-asserting dedup test (write FIRST per ECD-1 lesson f).
    /// When BOTH Evolution and GNOME Calendar are installed, ECD-2 must
    /// NOT emit — ECD-1 already covers the paths with
    /// `client = "evolution"`. Asserting `len() == 0` is not enough;
    /// we also assert the SET of ics_paths is empty, so a regression
    /// that emits with the wrong label (e.g. `"evolution"` instead of
    /// `"gnome_calendar"`) would still be caught.
    #[test]
    fn scan_gnome_calendar_alias_dedups_when_both_installed() {
        let home = TempHome::new();
        // Three ECD-1 calendars in a single source — the same fixture
        // shape as ECD-1's `finds_ics_per_source` so the orchestrator
        // and ECD-2 see the same paths.
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/a.ics",
            &evolution_ics("A"),
        );
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/b.ics",
            &evolution_ics("B"),
        );
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/c.ics",
            &evolution_ics("C"),
        );
        with_heuristics(
            /* gc_installed */ true,
            /* evolution_installed */ true,
            || {
                let gnome = scan_gnome_calendar_calendars(home.path(), &Platform::Linux);
                // Length assertion: ECD-2 must emit 0 entries (all
                // already covered by ECD-1).
                assert!(
                gnome.is_empty(),
                "when both Evolution and GNOME Calendar are installed, ECD-2 must emit 0 entries; got {gnome:?}"
            );
                // Value assertion: the SET of ics_paths must also be
                // empty. A regression that emits entries with the
                // wrong client label (e.g. still tagged "evolution")
                // would be caught here because the set would be
                // non-empty even if the count happened to be 0 in
                // some other test setup.
                let paths: std::collections::HashSet<PathBuf> =
                    gnome.iter().map(|c| c.ics_path.clone()).collect();
                assert!(
                    paths.is_empty(),
                    "ics_path set must be empty when dedup fires; got {paths:?}"
                );
                // Belt-and-braces: the dedup must also keep ECD-2's
                // entries out of the orchestrator's combined output.
                // (Simulate the orchestrator's flatten step.)
                let evolution = scan_evolution_calendars(home.path(), &Platform::Linux);
                let combined: Vec<_> = evolution.iter().chain(gnome.iter()).collect();
                let unique_paths: std::collections::HashSet<&PathBuf> =
                    combined.iter().map(|c| &c.ics_path).collect();
                assert_eq!(
                unique_paths.len(),
                3,
                "combined output must have exactly 3 unique ics_paths (ECD-1's 3, ECD-2 added 0)"
            );
                for c in &combined {
                    assert_eq!(
                    c.client, "evolution",
                    "all combined entries must carry the evolution label when Evolution is installed; got client={:?}",
                    c.client
                );
                }
            },
        );
    }

    /// Smoke test (write SECOND per ECD-1 lesson f). Locks in the
    /// test seam for non-Linux platforms — ECD-2 must short-circuit
    /// before any heuristic check runs (so the heuristic mocks can
    /// be set to `true` and the function still returns empty).
    #[test]
    fn scan_gnome_calendar_alias_empty_on_non_linux() {
        let home = TempHome::new();
        // Stage the fixture so a Linux run would find at least one
        // calendar — that way the "empty on non-Linux" assertion
        // can't be masked by a missing fixture.
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234/cal.ics",
            &evolution_ics("Cal"),
        );
        for platform in [
            Platform::Macos,
            Platform::Other("windows".to_string()),
            Platform::Other("freebsd".to_string()),
        ] {
            // Heuristics set to true to confirm the platform gate
            // fires BEFORE the heuristic check — otherwise the
            // test would only prove "false heuristic → empty".
            with_heuristics(true, true, || {
                let gnome = scan_gnome_calendar_calendars(home.path(), &platform);
                assert!(
                    gnome.is_empty(),
                    "non-Linux platform {platform:?} must return empty regardless of heuristics; got {gnome:?}"
                );
            });
        }
    }

    /// When Evolution is NOT installed but GNOME Calendar IS, ECD-2
    /// walks the same roots as ECD-1 and emits each `.ics` with
    /// `client = "gnome_calendar"` so the Ask step renders
    /// "GNOME Calendar" labels for these users.
    #[test]
    fn scan_gnome_calendar_alias_emits_only_when_gnome_calendar_installed() {
        let home = TempHome::new();
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/work.ics",
            &evolution_ics("Work"),
        );
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/personal.ics",
            &evolution_ics("Personal"),
        );
        with_heuristics(
            /* gc_installed */ true,
            /* evolution_installed */ false,
            || {
                let gnome = scan_gnome_calendar_calendars(home.path(), &Platform::Linux);
                assert_eq!(
                    gnome.len(),
                    2,
                    "expected 2 entries (one per .ics file); got {gnome:?}"
                );
                for cal in &gnome {
                    // The whole point of ECD-2: every entry carries the
                    // `gnome_calendar` label, not `evolution`.
                    assert_eq!(
                        cal.client, "gnome_calendar",
                        "all entries must carry the gnome_calendar client label; got {:?}",
                        cal.client
                    );
                    assert!(cal.ics_path.extension().and_then(|s| s.to_str()) == Some("ics"));
                    assert!(cal.profile.is_some());
                }
                // ics_paths must match the fixture.
                let paths: std::collections::HashSet<PathBuf> =
                    gnome.iter().map(|c| c.ics_path.clone()).collect();
                assert!(paths.iter().any(|p| p.ends_with("work.ics")));
                assert!(paths.iter().any(|p| p.ends_with("personal.ics")));
                // Display names survive the relabel.
                assert!(gnome
                    .iter()
                    .any(|c| c.display_name.as_deref() == Some("Work")));
                assert!(gnome
                    .iter()
                    .any(|c| c.display_name.as_deref() == Some("Personal")));
            },
        );
    }

    /// When NEITHER Evolution nor GNOME Calendar is installed, ECD-2
    /// returns empty — the heuristic gates both suppress emission.
    #[test]
    fn scan_gnome_calendar_alias_empty_when_neither_installed() {
        let home = TempHome::new();
        // Stage a real fixture — the "empty" outcome must come from
        // the heuristic gate, not from a missing fixture.
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/work.ics",
            &evolution_ics("Work"),
        );
        with_heuristics(
            /* gc_installed */ false,
            /* evolution_installed */ false,
            || {
                let gnome = scan_gnome_calendar_calendars(home.path(), &Platform::Linux);
                assert!(
                gnome.is_empty(),
                "neither Evolution nor GNOME Calendar installed must yield empty; got {gnome:?}"
            );
            },
        );
    }

    /// When only Evolution is installed (no GNOME Calendar), ECD-2
    /// returns empty — the heuristic gate suppresses emission because
    /// ECD-1 will already label the entries with `client = "evolution"`.
    /// This is the "evolution-only" counterpart of
    /// `emits_only_when_gnome_calendar_installed` and proves the
    /// client-label distinction is real (not "every .ics file ends up
    /// with whatever label was last assigned").
    #[test]
    fn scan_gnome_calendar_alias_emits_with_separate_client_label() {
        let home = TempHome::new();
        write_ics(
            &home,
            ".local/share/evolution/calendar/1234567890/work.ics",
            &evolution_ics("Work"),
        );
        // Only Evolution installed, no GNOME Calendar — the heuristic
        // gates suppress ECD-2 entirely. ECD-1 still emits with the
        // `evolution` label (proving the orchestrator's combined
        // output is correctly partitioned: ECD-1 rows say
        // "evolution", ECD-2 rows are absent).
        with_heuristics(
            /* gc_installed */ false,
            /* evolution_installed */ true,
            || {
                let gnome = scan_gnome_calendar_calendars(home.path(), &Platform::Linux);
                assert!(
                gnome.is_empty(),
                "only Evolution installed (no GNOME Calendar) must yield empty ECD-2 output; got {gnome:?}"
            );
                // ECD-1 still emits with the evolution label — proving
                // the "separate client label" contract: when ECD-2 DOES
                // emit (in the only-gnome-calendar case), it uses a
                // distinct label from ECD-1.
                let evolution = scan_evolution_calendars(home.path(), &Platform::Linux);
                assert_eq!(evolution.len(), 1, "ECD-1 must still emit 1 entry");
                assert_eq!(evolution[0].client, "evolution");
            },
        );
    }

    // -----------------------------------------------------------------------
    // ECD-4 — KOrganizer + Outlook UX-fallback probes
    //
    // KOrganizer (Linux) and Outlook (Windows) are intentionally
    // UX-fallback ONLY: their on-disk artifact discovery is too
    // complex for v1 (KOrganizer: Akonadi resource backends are
    // heterogeneous — file / SQLite / MySQL; Outlook: Windows
    // registry + MAPI). The detectors return `Available` + a hint
    // string asking the user to export the calendar themselves
    // (File → Export → iCalendar for KOrganizer; Outlook File →
    // Save As → iCalendar Format per calendar). The existing
    // manual `.ics` path input stays as-is — the fallback is
    // purely a UX hint surfaced via the candidate's `notes` field.
    //
    // Test pattern mirrors the ECD-2 heuristic-mock test helper
    // (see `with_heuristics` above). The `KORGANIZER_PRESENT` and
    // `OUTLOOK_PRESENT` slots are `None` in production (the
    // functions fall through to `which korganizer` /
    // `Path::is_file(OUTLOOK.EXE)` respectively) and `Some(b)` in
    // tests.
    // -----------------------------------------------------------------------

    /// Run `body` with the ECD-4 heuristic mocks set to
    /// `korganizer_installed` and `outlook_installed`. The guard
    /// restores the previous values on drop (panic-safe), so
    /// subsequent tests start from the "no mock" default.
    fn with_ecd4_heuristics<F>(korganizer_installed: bool, outlook_installed: bool, body: F)
    where
        F: FnOnce(),
    {
        let prev_ko = KORGANIZER_PRESENT
            .lock()
            .expect("heuristic mutex")
            .replace(korganizer_installed);
        let prev_ol = OUTLOOK_PRESENT
            .lock()
            .expect("heuristic mutex")
            .replace(outlook_installed);
        let _restore = Ecd4HeuristicsGuard { prev_ko, prev_ol };
        body();
    }

    /// RAII guard that restores the ECD-4 heuristic mocks on drop.
    struct Ecd4HeuristicsGuard {
        prev_ko: Option<bool>,
        prev_ol: Option<bool>,
    }

    impl Drop for Ecd4HeuristicsGuard {
        fn drop(&mut self) {
            let mut ko = KORGANIZER_PRESENT.lock().expect("heuristic mutex");
            *ko = self.prev_ko;
            let mut ol = OUTLOOK_PRESENT.lock().expect("heuristic mutex");
            *ol = self.prev_ol;
        }
    }

    /// Smoke test (write FIRST per Pitfall #127). When KOrganizer
    /// is on the user's PATH and the platform is Linux,
    /// `scan_korganizer` must return `Available` with
    /// `CommandExists { binary: "korganizer", ... }` evidence +
    /// the UX-fallback hint in `notes`. Mirrors the
    /// `scan_gnome_calendar_calendars` heuristic-mock pattern.
    #[test]
    fn scan_korganizer_smoke_returns_available_on_linux_when_on_path() {
        with_ecd4_heuristics(
            /* korganizer_installed */ true,
            /* outlook_installed */ false,
            || {
                let c = scan_korganizer(&Platform::Linux);
                assert_eq!(
                    c.status,
                    CollectorStatus::Available,
                    "KOrganizer on Linux + on PATH must be Available; got {:?}",
                    c.status
                );
                assert_eq!(c.collector_id, "korganizer");
                match &c.evidence {
                    EvidenceKind::CommandExists { binary, path } => {
                        assert_eq!(binary, "korganizer");
                        assert!(
                            !path.as_os_str().is_empty(),
                            "CommandExists path must be non-empty for the available case"
                        );
                    }
                    other => panic!("expected CommandExists evidence; got {other:?}"),
                }
            },
        );
    }

    /// Count-asserting: KOrganizer must return `Unavailable` on
    /// every non-Linux platform (KDE PIM has no Windows / macOS
    /// build) with a platform-skip note. The mock is set to
    /// `true` to confirm the platform gate runs BEFORE the
    /// heuristic check (mirrors ECD-2's "empty on non-Linux"
    /// test pattern).
    #[test]
    fn scan_korganizer_unavailable_on_non_linux() {
        for platform in [
            Platform::Macos,
            Platform::Other("windows".to_string()),
            Platform::Other("freebsd".to_string()),
        ] {
            with_ecd4_heuristics(
                /* korganizer_installed */ true,
                /* outlook_installed */ false,
                || {
                    let c = scan_korganizer(&platform);
                    assert_eq!(
                        c.status,
                        CollectorStatus::Unavailable,
                        "KOrganizer on non-Linux {platform:?} must be Unavailable; got {:?}",
                        c.status
                    );
                    let notes = c.notes.as_deref().unwrap_or("");
                    assert!(
                        notes.contains("Linux-only"),
                        "non-Linux KOrganizer notes must mention Linux-only; got {notes:?}"
                    );
                },
            );
        }
    }

    /// Count-asserting: Outlook must return `Available` on
    /// `Platform::Other("windows")` when OUTLOOK.EXE is present
    /// + `Unavailable` when the mock is `false`. Confirms the
    /// production probe is gated by the mock slot in test mode.
    #[test]
    fn scan_outlook_available_on_windows_when_present() {
        let windows = Platform::Other("windows".to_string());
        with_ecd4_heuristics(
            /* korganizer_installed */ false,
            /* outlook_installed */ true,
            || {
                let c = scan_outlook(&windows);
                assert_eq!(
                    c.status,
                    CollectorStatus::Available,
                    "Outlook on Windows + OUTLOOK.EXE present must be Available; got {:?}",
                    c.status
                );
                assert_eq!(c.collector_id, "outlook");
            },
        );
        with_ecd4_heuristics(
            /* korganizer_installed */ false,
            /* outlook_installed */ false,
            || {
                let c = scan_outlook(&windows);
                assert_eq!(
                    c.status,
                    CollectorStatus::Unavailable,
                    "Outlook on Windows + OUTLOOK.EXE missing must be Unavailable; got {:?}",
                    c.status
                );
                let notes = c.notes.as_deref().unwrap_or("");
                assert!(
                    notes.contains("OUTLOOK.EXE"),
                    "Windows + missing OUTLOOK.EXE notes must mention OUTLOOK.EXE; got {notes:?}"
                );
            },
        );
    }

    /// Count-asserting: Outlook must return `Unavailable` on
    /// every non-Windows platform with the platform-skip note.
    /// The Outlook mock is set to `true` to confirm the platform
    /// gate runs BEFORE the heuristic check.
    #[test]
    fn scan_outlook_unavailable_on_non_windows() {
        for platform in [
            Platform::Linux,
            Platform::Macos,
            Platform::Other("freebsd".to_string()),
        ] {
            with_ecd4_heuristics(
                /* korganizer_installed */ false,
                /* outlook_installed */ true,
                || {
                    let c = scan_outlook(&platform);
                    assert_eq!(
                        c.status,
                        CollectorStatus::Unavailable,
                        "Outlook on non-Windows {platform:?} must be Unavailable; got {:?}",
                        c.status
                    );
                    let notes = c.notes.as_deref().unwrap_or("");
                    assert!(
                        notes.contains("Windows-only"),
                        "non-Windows Outlook notes must mention Windows-only; got {notes:?}"
                    );
                },
            );
        }
    }

    /// Value-asserting: the per-OS availability notes must
    /// contain the right UX-fallback hint text. The KOrganizer
    /// hint mentions "File → Export → iCalendar" (KDE
    /// convention); the Outlook hint mentions "iCalendar
    /// Format" (Microsoft Office Save As dialog). Asserting the
    /// EXACT substring guards against a copy-paste regression
    /// where one hint accidentally replaces the other.
    #[test]
    fn scan_korganizer_and_outlook_notes_carry_ux_fallback_hints() {
        with_ecd4_heuristics(
            /* korganizer_installed */ true,
            /* outlook_installed */ true,
            || {
                let ko = scan_korganizer(&Platform::Linux);
                let ko_notes = ko.notes.as_deref().unwrap_or("");
                assert!(
                    ko_notes.contains("KOrganizer is installed"),
                    "KOrganizer notes must announce detection; got {ko_notes:?}"
                );
                assert!(
                    ko_notes.contains("File → Export → iCalendar"),
                    "KOrganizer notes must contain the File → Export → iCalendar hint; got {ko_notes:?}"
                );
                assert!(
                    ko_notes.contains("paste the path below"),
                    "KOrganizer notes must mention the manual .ics path input; got {ko_notes:?}"
                );

                let windows = Platform::Other("windows".to_string());
                let ol = scan_outlook(&windows);
                let ol_notes = ol.notes.as_deref().unwrap_or("");
                assert!(
                    ol_notes.contains("Outlook is installed"),
                    "Outlook notes must announce detection; got {ol_notes:?}"
                );
                assert!(
                    ol_notes.contains("iCalendar Format"),
                    "Outlook notes must mention the iCalendar Format save dialog; got {ol_notes:?}"
                );
                assert!(
                    ol_notes.contains("per-calendar"),
                    "Outlook notes must surface the per-calendar export scope; got {ol_notes:?}"
                );
            },
        );
    }
}
