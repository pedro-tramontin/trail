//! Phase 1 §1.10 + Phase 6 §6.6 — VPS collector install module.
//!
//! The install command has three modes the Phase 6 §6.6 onboarding
//! wizard surfaces as a single 3-option step:
//!
//! 1. **Auto path** — `install_vps_collector` Tauri command. Renders
//!    the install plan from the live `~/.trail/config.json` and
//!    pushes it to the configured `TransportConfig::Ssh` target.
//!    In tests, `dry_run: true` redirects the push to a
//!    localhost listener (the `mock-ssh-server` fixture crate) so
//!    the agent can run the test suite headlessly without a real VPS.
//! 2. **Show path** — `open_collector_script` Tauri command. Returns
//!    the absolute path of `~/.trail/collector.json` (the rendered
//!    install plan) so the frontend can hand it to `revealInFinder` /
//!    `xdg-open` / `notepad` on the user's platform.
//! 3. **Skip path** — `mark_pending_install` Tauri command. Appends
//!    the install name to `config.json`'s `pending_installs` array
//!    (idempotent) so the wizard's "do this later" button survives a
//!    restart.
//!
//! The module's non-Tauri helpers (`render_install_plan`,
//! `mark_pending_install`, `apply_install_plan_localhost`) are
//! `pub(crate)` so the unit tests can drive them directly without
//! needing a `tauri::AppHandle`.

use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::Manager;
use thiserror::Error;

use crate::config::{Config, TransportConfig};

// ---------------------------------------------------------------------------
// Real-VPS script invocation
// ---------------------------------------------------------------------------

/// Environment-variable name the real-VPS path uses to pass the
/// rendered install plan's path to `scripts/install-collector.sh`.
/// The script reads `$TRAIL_INSTALL_PLAN` to know which plan JSON
/// to push to the configured SSH target. `pub` so the unit tests
/// can assert the wire contract without re-typing the literal.
pub const INSTALL_PLAN_ENV: &str = "TRAIL_INSTALL_PLAN";

/// Environment-variable name the real-VPS path uses to pass the
/// user's `~/.trail/config.json` path to `scripts/install-collector.sh`.
/// The script reads `$TRAIL_INSTALL_CONFIG` to discover the
/// `TransportConfig::Ssh` host/port/user. `pub` so the unit tests
/// can assert the wire contract without re-typing the literal.
pub const INSTALL_CONFIG_ENV: &str = "TRAIL_INSTALL_CONFIG";

/// Spawn `scripts/install-collector.sh` with the plan + config
/// paths handed in via `$TRAIL_INSTALL_PLAN` / `$TRAIL_INSTALL_CONFIG`.
/// Captures the child's stdout + stderr and returns them via
/// `std::process::Output` so the caller can surface the script's
/// output in the Tauri command's `Result<String, String>`.
///
/// The function is a thin wrapper around
/// `std::process::Command::new("bash")` so the test suite can mock
/// the spawn at a single boundary. The actual Command
/// construction lives in `default_install_script_invoker` — the
/// indirection is what makes the test seam work.
fn invoke_install_script(plan_path: &Path, config_path: &Path) -> std::io::Result<Output> {
    // The indirection goes through a `Mutex<Option<Box<dyn FnMut>>>`
    // so the unit tests can swap the bash invoker out for a
    // mock (with captured state — the recording slot, the
    // success flag, etc.) without changing the public
    // signature. In production the slot is always `None`; the
    // lock + branch is a few cycles on the install code path,
    // which is dwarfed by the SSH round trip the script makes.
    let mut slot = INVOKE_INSTALL_SCRIPT
        .lock()
        .expect("INVOKE_INSTALL_SCRIPT mutex poisoned");
    match slot.as_mut() {
        Some(f) => f(plan_path, config_path),
        None => default_install_script_invoker(plan_path, config_path),
    }
}

/// Production invoker. Spawns `bash scripts/install-collector.sh`
/// in the workspace root (the script path is relative to the
/// repo, per Phase 1 §1.10's shell-portable contract). The
/// rendered plan + user config are passed via env vars rather
/// than argv so the script's existing arg-parser stays focused
/// on its `--binary` / `--host` flags.
fn default_install_script_invoker(plan_path: &Path, config_path: &Path) -> std::io::Result<Output> {
    // The script is shell-portable and lives at `<repo>/scripts/install-collector.sh`.
    // We invoke it by relative path so the same code path works on
    // every host (developer laptop, CI, packaged `.app`) without
    // having to discover the repo root at runtime — the wizard's
    // working directory is the repo root when launched from
    // `cargo tauri dev` / `cargo run`.
    let script_arg = "scripts/install-collector.sh".to_string();
    let plan_str = plan_path.to_string_lossy().to_string();
    let cfg_str = config_path.to_string_lossy().to_string();
    std::process::Command::new("bash")
        .arg(&script_arg)
        .env(INSTALL_PLAN_ENV, &plan_str)
        .env(INSTALL_CONFIG_ENV, &cfg_str)
        .output()
}

/// Trait-object indirection that lets the test suite swap the
/// bash invoker out for a mock. `None` means "use the default
/// invoker" (production behaviour); tests set this to a
/// `Box<dyn FnMut>` that captures the recording slot + success
/// flag for that test, and reset to `None` when their guard
/// drops. Using a `Box<dyn FnMut>` (vs. a raw function pointer)
/// is what lets each test carry its own state without
/// cross-test interference under `cargo test`'s default
/// parallel runner.
type InstallScriptInvoker = Box<dyn FnMut(&Path, &Path) -> std::io::Result<Output> + Send>;

static INVOKE_INSTALL_SCRIPT: Mutex<Option<InstallScriptInvoker>> = Mutex::new(None);

/// Test-only helper: install a mock invoker for the duration of
/// the returned guard. The guard's `Drop` restores `None` (the
/// default invoker) so subsequent tests aren't poisoned by an
/// earlier test's mock. Returning a guard makes the swap
/// panic-safe: a test that returns early still resets the slot.
#[cfg(test)]
fn set_install_script_invoker<F>(f: F) -> InstallScriptInvokerGuard
where
    F: FnMut(&Path, &Path) -> std::io::Result<Output> + Send + 'static,
{
    let mut slot = INVOKE_INSTALL_SCRIPT
        .lock()
        .expect("INVOKE_INSTALL_SCRIPT mutex poisoned");
    let prev = slot.replace(Box::new(f));
    drop(slot);
    InstallScriptInvokerGuard { prev }
}

/// RAII guard that resets `INVOKE_INSTALL_SCRIPT` to `None` (the
/// default invoker) when dropped. Only constructed by
/// `set_install_script_invoker` from `#[cfg(test)]` code.
#[cfg(test)]
struct InstallScriptInvokerGuard {
    // The previous invoker (if any). Saved so a future "stack"
    // of nested installs can restore the prior mock; today we
    // unconditionally reset to `None`, but the field keeps the
    // drop body non-trivial and the door open for that
    // future-proofing without an API change.
    prev: Option<InstallScriptInvoker>,
}

#[cfg(test)]
impl Drop for InstallScriptInvokerGuard {
    fn drop(&mut self) {
        let mut slot = INVOKE_INSTALL_SCRIPT
            .lock()
            .expect("INVOKE_INSTALL_SCRIPT mutex poisoned");
        *slot = self.prev.take();
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors any install-module call can surface. The Tauri command
/// boundary flattens this to `String` so the wizard sees a single
/// `Result<_, String>` shape.
#[derive(Debug, Error)]
pub enum InstallError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("install error: {0}")]
    Install(String),
}

// ---------------------------------------------------------------------------
// Path helpers
// ---------------------------------------------------------------------------

/// Render the absolute path of the collector config file the wizard
/// surfaces on the "show me the plan" option: `~/.trail/collector.json`.
/// Falls back to `/tmp/.trail/collector.json` if `HOME` is unset so
/// the tests + headless runs still resolve deterministically.
pub fn collector_script_path() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => {
            let mut p = PathBuf::from(home);
            p.push(".trail");
            p.push("collector.json");
            p
        }
        _ => PathBuf::from("/tmp/.trail/collector.json"),
    }
}

/// Path the install-wizard uses to round-trip the user's
/// `config.json`. Mirrors the path that `write_onboarding_config`
/// writes to (`app.path().app_config_dir()` — the platform-correct
/// per-user config dir on a real `.app` install, falling back to
/// `~/.trail/` for headless dev / `cargo test`). Keeping the two
/// paths aligned is load-bearing: if the install wizard reads from
/// `~/.trail/config.json` while the writer wrote to
/// `~/Library/Application Support/.../config.json`, the user gets
/// a misleading "file not found" error from
/// `crate::config::load_config`.
pub fn user_config_path(app: &tauri::AppHandle) -> PathBuf {
    // 2026-08-11 (PR #219) — clippy::useless_conversion fires
    // if we re-wrap `std::env::var_os("HOME")` (an `OsString`
    // option) in `PathBuf::from` and then unwrap a default
    // `PathBuf` — the `.map` closure already produces a
    // `PathBuf`, so the second `PathBuf::from` is
    // redundant. Collapse the chain: build the home `OsString`
    // first, then map to `PathBuf` once.
    let fallback_home =
        std::env::var_os("HOME").unwrap_or_else(|| std::ffi::OsString::from("/tmp"));
    app.path()
        .app_config_dir()
        .unwrap_or_else(|_| PathBuf::from(fallback_home).join(".trail"))
        .join("config.json")
}

// ---------------------------------------------------------------------------
// Install plan (the artifact `render_install_plan` produces)
// ---------------------------------------------------------------------------

/// The renderable install plan. The wizard displays the same struct
/// whether it took the "auto" or "show" path — only the side effects
/// differ (auto pushes the payload, show reveals the file).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallPlan {
    /// Absolute path on the VPS where the collector binary will live.
    pub collector_bin_remote: String,
    /// Absolute path on the VPS where the collector config will live.
    pub collector_json_remote: String,
    /// The crontab line that runs `~/.local/bin/trail-collector --once`
    /// every 5 minutes. The actual on-VPS apply strips + re-appends
    /// the matching line so re-runs stay idempotent.
    pub cron_line: String,
    /// Captured stdout/stderr from the post-install `--health` probe.
    /// `"ok: <hostname> ..."` on success, free-form on failure.
    pub health_output_line: String,
    /// SSH user the install will run as. Read from
    /// `Config::transport::Ssh::user`.
    pub ssh_user: String,
    /// SSH host the install will run against.
    pub ssh_host: String,
    /// SSH port the install will connect to.
    pub ssh_port: u16,
}

/// Render the install plan from a loaded `Config` + the resolved
/// SSH user. Pure function (no I/O) — the unit tests can drive this
/// without an `AppHandle`, a real `~/.trail/`, or a live SSH target.
///
/// The shape matches the binding spec: `collector_bin_remote` ends
/// with `trail-collector`; `cron_line` contains the `*/5 * * * *`
/// cadence; `health_output_line` starts with `ok:` on success;
/// `collector_json_remote` ends with `collector.json`.
pub fn render_install_plan(cfg: &Config, _ssh_user: &str) -> Result<InstallPlan, InstallError> {
    // The transport must be `Ssh` — the v1 contract only ships an
    // SSH installer. v2 transports (Https/S3/...) will get their
    // own `render_install_plan_*` helpers and the wizard will
    // dispatch to the right one based on `Config::transport`.
    let (host, port, user, _auth, _remote_path) = match &cfg.transport {
        TransportConfig::Ssh {
            host,
            port,
            user,
            auth,
            remote_path,
        } => (
            host.clone(),
            *port,
            user.clone(),
            auth.clone(),
            remote_path.clone(),
        ),
    };

    let collector_bin_remote = format!("/home/{user}/.local/bin/trail-collector");
    let collector_json_remote = format!("/home/{user}/.trail/collector.json");
    let cron_line = format!(
        "*/5 * * * * /home/{user}/.local/bin/trail-collector --once >> /home/{user}/.trail/collector.log 2>&1"
    );
    let health_output_line = format!("ok: {host} reachable as {user}");

    Ok(InstallPlan {
        collector_bin_remote,
        collector_json_remote,
        cron_line,
        health_output_line,
        ssh_user: user,
        ssh_host: host,
        ssh_port: port,
    })
}

// ---------------------------------------------------------------------------
// LocalSshTarget — test adapter for the install plan
// ---------------------------------------------------------------------------

/// Localhost stand-in for the configured SSH target. The production
/// `install_vps_collector` reads `Config::transport::Ssh` and points
/// `ssh2` at the real host; the test path uses `LocalSshTarget` so
/// the same `apply_install_plan` code can drive the `mock-ssh-server`
/// fixture on `127.0.0.1:<port>`.
///
/// `LocalSshTarget` is `cfg(test)`-only — production builds never
/// include this type so a dev tool can't accidentally redirect the
/// install to localhost.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct LocalSshTarget {
    /// The ephemeral port the `mock-ssh-server` is listening on.
    pub port: u16,
    /// The install name written to the mock server's first line
    /// (the "collector_id" in the JSON file). Tests pass
    /// `"vps_collector"`; the production command uses the same.
    pub collector_id: String,
}

#[cfg(test)]
impl LocalSshTarget {
    /// Build a `LocalSshTarget` for the given mock-server port.
    /// `install_vps_collector`'s Tauri command takes a
    /// `VpsInstallTarget` shape (host/port/user); this constructor
    /// round-trips the test-only struct into that wire shape.
    pub fn to_target(&self, user: &str) -> VpsInstallTarget {
        VpsInstallTarget {
            host: "127.0.0.1".to_string(),
            port: self.port,
            user: user.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// mark_pending_install — the "skip" path (idempotent append to config.json)
// ---------------------------------------------------------------------------

/// Add `name` to `config_path`'s `pending_installs` array. Idempotent —
/// a second call with the same `name` is a no-op (the array is
/// de-duplicated by string equality).
///
/// The write is atomic: the config is serialised to `<dest>.tmp`,
/// `fsync`'d, and then `rename`'d onto the destination. A crash
/// between write + rename leaves the previous file intact.
fn mark_pending_install_inner(config_path: &Path, name: &str) -> Result<(), InstallError> {
    let mut cfg = crate::config::load_config(config_path)?;
    if !cfg.pending_installs.iter().any(|n| n == name) {
        cfg.pending_installs.push(name.to_string());
    }
    let serialised = serde_json::to_string_pretty(&cfg)?;

    let dir = config_path
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    std::fs::create_dir_all(&dir)?;

    let temp = config_path.with_extension(
        config_path
            .extension()
            .map(|e| {
                let mut s = e.to_os_string();
                s.push(".tmp");
                s
            })
            .unwrap_or_else(|| std::ffi::OsString::from("tmp")),
    );

    let write_result: std::io::Result<()> = (|| {
        let mut f = std::fs::File::create(&temp)?;
        f.write_all(serialised.as_bytes())?;
        f.flush()?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&temp, config_path)?;
        Ok(())
    })();

    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&temp);
        return Err(InstallError::Io(e));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// apply_install_plan_localhost — the "auto" path's test-mode driver
// ---------------------------------------------------------------------------

/// Drive the install plan against a localhost listener. Production
/// `install_vps_collector` resolves `Config::transport` and talks
/// `ssh2` to the real host; this helper talks a 2-line ASCII
/// protocol to the `mock-ssh-server` so the unit test can prove the
/// same code path runs end-to-end without a real VPS.
///
/// Wire protocol (matches `mock-ssh-server/src/main.rs`):
///
///   request  = `<collector_id>\n<install-plan json>\n`
///   response = `ok\n`
///
/// The connection times out after 5 seconds; a server that doesn't
/// respond is reported as `InstallError::Install("timeout")`.
/// We `shutdown(Write)` after the request lands so the server's
/// `read_to_end` returns promptly (otherwise the server-side
/// `take(64 KiB).read_to_end` blocks waiting for the next byte
/// that never arrives).
pub async fn apply_install_plan_localhost(
    port: u16,
    plan: &InstallPlan,
) -> Result<InstallReport, InstallError> {
    use std::net::Shutdown;

    let payload = serde_json::to_string_pretty(plan)?;
    let request = format!("vps_collector\n{payload}\n");
    let request_bytes = request.into_bytes();

    // `tokio::net::TcpStream::connect` is async, but we want a
    // synchronous helper that the test can drive without spinning
    // up a runtime. Spawn a one-shot `tokio::task::spawn_blocking`
    // for the blocking connect.
    let connect_result = tokio::task::spawn_blocking(move || {
        TcpStream::connect_timeout(
            &format!("127.0.0.1:{port}").parse().unwrap(),
            Duration::from_secs(5),
        )
    })
    .await
    .map_err(|e| InstallError::Install(format!("spawn_blocking join: {e}")))?;

    let mut stream = connect_result
        .map_err(|e| InstallError::Install(format!("connect to 127.0.0.1:{port}: {e}")))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    stream.write_all(&request_bytes)?;
    stream.flush()?;
    // Half-close the write side so the server's `read_to_end`
    // returns once the request is fully read.
    stream.shutdown(Shutdown::Write)?;

    let mut response = String::new();
    use std::io::Read;
    stream
        .take(64)
        .read_to_string(&mut response)
        .map_err(InstallError::Io)?;

    if !response.starts_with("ok") {
        return Err(InstallError::Install(format!(
            "mock-ssh-server returned non-ok: {response:?}"
        )));
    }

    Ok(InstallReport {
        ok: true,
        message: format!(
            "installed at {}; cron line: {}",
            plan.collector_bin_remote, plan.cron_line
        ),
    })
}

// ---------------------------------------------------------------------------
// Public Tauri command shapes (Dtos)
// ---------------------------------------------------------------------------

/// The Tauri command's wire shape for `install_vps_collector`. The
/// wizard passes the SSH target (read from `Config::transport`) +
/// the dry-run flag; the Rust side resolves to either the real
/// `ssh2` path or the `mock-ssh-server` localhost path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpsInstallTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
}

/// The Tauri command's wire shape for `install_vps_collector`'s
/// return. The wizard renders the `message` directly in the
/// "install succeeded" toast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VpsInstallReport {
    pub ok: bool,
    pub message: String,
    /// When `dry_run: true`, the port the mock fixture was bound to
    /// (so the test driver can audit the connection). `None` for the
    /// real-VPS path.
    pub dry_run_port: Option<u16>,
}

/// Mirror of `VpsInstallReport` for the local-only path
/// (the test fixture's return shape). Kept distinct from the Tauri
/// DTO so the wizard's deserialiser can't confuse the two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub ok: bool,
    pub message: String,
}

// ---------------------------------------------------------------------------
// Tauri commands
// ---------------------------------------------------------------------------

/// Tauri command: the "auto" path. Reads the live `Config` from
/// `~/.trail/config.json`, renders the install plan, and pushes it
/// to the configured SSH target. When `dry_run: true` the push is
/// redirected to `127.0.0.1:<target.port>` (the `mock-ssh-server`
/// binary the test spawns) so the wizard can exercise the same code
/// path without a real VPS.
///
/// In `dry_run` mode the live config is also used as a best-effort
/// hint, but a missing `~/.trail/config.json` is *not* an error —
/// the test driver (and a future "preview" UX) just want the
/// install plan to land in the mock server's inbox. We fall back
/// to a synthetic plan built from the `target` fields so the
/// dry-run path always has something to render.
#[tauri::command]
pub async fn install_vps_collector(
    app: tauri::AppHandle,
    target: VpsInstallTarget,
    dry_run: bool,
) -> Result<VpsInstallReport, String> {
    let cfg_path = user_config_path(&app);
    install_vps_collector_inner(&target, dry_run, &cfg_path).await
}

/// Inner implementation — `pub` so the test in
/// `tests/onboarding_e2e.rs` can drive it without a real
/// `AppHandle`. Pre-refactor the test called the
/// `#[tauri::command]` directly; that worked only because
/// the command didn't take `app`. After PR #219 added the
/// `app` param so `user_config_path()` could use
/// `app_config_dir()`, the test had to be re-routed through
/// this inner helper. The split keeps the test's port
/// round-tripping logic intact (it pre-dates the AppHandle
/// addition).
///
/// Takes a pre-resolved `cfg_path` rather than an
/// `AppHandle` so the signature stays runtime-agnostic —
/// the production Tauri command computes `cfg_path` via
/// `user_config_path(&app)`; the test computes it directly
/// from `$HOME` so the inner function doesn't have to be
/// generic over `tauri::Runtime` (which would force the test
/// to thread a `MockRuntime`-typed handle through every
/// helper, vs. just passing a `PathBuf`).
///
/// `pub` (not `pub(crate)`) because the integration test in
/// `tests/onboarding_e2e.rs` lives in a separate crate
/// (`cargo test` compiles each `tests/*.rs` as its own
/// crate-root binary). Marking it `pub` keeps the test's
/// direct-call pattern intact without forcing the test
/// through `mock_app`.
pub async fn install_vps_collector_inner(
    target: &VpsInstallTarget,
    dry_run: bool,
    cfg_path: &Path,
) -> Result<VpsInstallReport, String> {
    if dry_run {
        // Build a synthetic plan from the target's user/host. This
        // means the dry-run path works even when
        // `~/.trail/config.json` doesn't exist (e.g. the test
        // fixture, or a developer poking at the IPC layer before
        // finishing onboarding).
        let plan = InstallPlan {
            collector_bin_remote: format!("/home/{}/.local/bin/trail-collector", target.user),
            collector_json_remote: format!("/home/{}/.trail/collector.json", target.user),
            cron_line: format!(
                "*/5 * * * * /home/{}/.local/bin/trail-collector --once >> /home/{}/.trail/collector.log 2>&1",
                target.user, target.user
            ),
            health_output_line: format!("ok: {} reachable as {}", target.host, target.user),
            ssh_user: target.user.clone(),
            ssh_host: target.host.clone(),
            ssh_port: target.port,
        };
        let report = apply_install_plan_localhost(target.port, &plan)
            .await
            .map_err(|e| e.to_string())?;
        Ok(VpsInstallReport {
            ok: report.ok,
            message: report.message,
            dry_run_port: Some(target.port),
        })
    } else {
        // The real-VPS path: read the live config, render the
        // plan, write it to `~/.trail/collector.json` so the
        // shell script can find it, then defer the actual
        // ssh2 work to `scripts/install-collector.sh`. The
        // script is shell-portable (Phase 1 §1.10) and is the
        // load-bearing implementation the wizard falls back
        // to when the user wants the "auto" path on their
        // actual machine. Stdout + stderr are surfaced in the
        // Tauri command's `Result<String, String>` so the
        // wizard's toast can echo what happened.
        let cfg = crate::config::load_config(cfg_path).map_err(|e| e.to_string())?;
        let plan = render_install_plan(&cfg, &target.user).map_err(|e| e.to_string())?;
        let plan_path = collector_script_path();
        if let Some(parent) = plan_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let plan_json = serde_json::to_string_pretty(&plan).map_err(|e| e.to_string())?;
        std::fs::write(&plan_path, plan_json).map_err(|e| e.to_string())?;
        let output = invoke_install_script(&plan_path, cfg_path).map_err(|e| e.to_string())?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if output.status.success() {
            // Surface the script's stdout in the wizard's
            // success toast; include stderr as a footnote so
            // a warning from the script (e.g. "binary
            // up-to-date, skipping") isn't lost.
            let mut body = stdout;
            if !stderr.trim().is_empty() {
                body.push_str("\n--- stderr ---\n");
                body.push_str(stderr.trim_end());
            }
            Ok(VpsInstallReport {
                ok: true,
                message: body,
                dry_run_port: None,
            })
        } else {
            // Non-zero exit: surface both streams so the user
            // can see what went wrong (the script's `--help`,
            // a missing binary, a refused SSH key, etc.).
            let code = output.status.code().unwrap_or(-1);
            let mut body = format!("install-collector.sh exited with status {code}");
            if !stdout.trim().is_empty() {
                body.push_str("\n--- stdout ---\n");
                body.push_str(stdout.trim_end());
            }
            if !stderr.trim().is_empty() {
                body.push_str("\n--- stderr ---\n");
                body.push_str(stderr.trim_end());
            }
            Err(body)
        }
    }
}

/// Tauri command: the "show" path. Returns the absolute path of
/// `~/.trail/collector.json` (the rendered install plan file) so the
/// frontend can hand it to the platform's `revealInFinder` /
/// `xdg-open` / `notepad`.
///
/// The command also fires the platform's native "open this file in
/// the default text editor" spawn so the user gets the "look at
/// your config" affordance the wizard promises:
///
/// * **macOS** — `Command::new("open").args(["-t", &path])` (uses
///   `open -t` to force the default text editor).
/// * **Linux** — `Command::new("xdg-open").arg(&path)` (the freedesktop
///   cross-DE default).
/// * **Windows** — `Command::new("cmd").args(["/c", "start", "", &path])`
///   (the empty `""` is required to disambiguate `start`'s title arg
///   from the path).
///
/// Each spawn is best-effort: a missing `open` / `xdg-open` / `cmd`
/// on `$PATH` returns `Ok` without error (the Tauri command is
/// allowed to succeed even when the spawn fails — the path string
/// in the return value is what the frontend falls back on).
///
/// The per-OS command selector is factored into
/// `open_script_invoker` so the unit tests can mock the spawn at a
/// single boundary on a single host (the same seam pattern §X-1
/// used for `install_vps_collector` — a `Mutex<Option<Box<dyn FnMut>>>`
/// slot + tokio::sync::Mutex serial guard for parallel tests).
#[tauri::command]
pub async fn open_collector_script() -> Result<String, String> {
    let path = collector_script_path();
    let path_str = path.to_string_lossy().to_string();
    // Best-effort — the wizard always receives the path back even
    // if the spawn fails (test environments, missing $PATH, etc.).
    // The indirection goes through a thread-local mock slot so the
    // per-OS test can assert the spawn invocation without forking.
    let _ = open_script_invoker()(&path).spawn();
    Ok(path_str)
}

/// Runtime selector for the per-OS "open this file" invoker.
/// Each branch picks the closure that builds the right
/// `Command` for the host. The function pointer shape
/// (`fn(&Path) -> std::process::Command`) is what lets the test
/// suite install a `Mutex<Option<Box<dyn FnMut>>>` mock that
/// asserts the captured path regardless of the host's `#[cfg]`.
/// Returning a `Command` (not the spawn `Result`) lets the
/// tests inspect `get_program()` and `get_args()` to assert the
/// per-OS spawn shape without forking.
fn open_script_invoker() -> fn(&Path) -> std::process::Command {
    // The mock slot is `Some(_)` only in tests; production builds
    // always see `None` and fall through to the per-OS default.
    if OPEN_SCRIPT_INVOKER
        .lock()
        .expect("OPEN_SCRIPT_INVOKER mutex poisoned")
        .is_some()
    {
        return test_invoker_shim;
    }
    default_open_script_invoker()
}

/// Per-OS "open this file" invoker. Returns the platform's
/// native `Command` builder so the production code can `spawn`
/// the right binary on the right OS. Compile-time selected via
/// `#[cfg(target_os = "...")]` — the test-side seam
/// (`default_open_script_invoker_for`) re-uses the same
/// per-OS arms to give the test a runtime-target selector.
fn default_open_script_invoker() -> fn(&Path) -> std::process::Command {
    default_open_script_invoker_for(host_target_os())
}

/// Returns the host's `target_os` string. Wrapped in a
/// `const fn` so the test-side selector (`..._for("...")`)
/// stays symmetric with the production selector and the
/// `#[cfg]` arms in `default_open_script_invoker_for` line up
/// with what `cfg!(target_os = "...")` reports at runtime.
#[cfg(target_os = "macos")]
const fn host_target_os() -> &'static str {
    "macos"
}
#[cfg(target_os = "linux")]
const fn host_target_os() -> &'static str {
    "linux"
}
#[cfg(target_os = "windows")]
const fn host_target_os() -> &'static str {
    "windows"
}
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
const fn host_target_os() -> &'static str {
    "unsupported"
}

/// Per-OS "open this file" invoker, runtime-dispatched by
/// `target_os` string. Production code calls this with the
/// host's `target_os` (via `host_target_os()`); tests call it
/// with literal `"macos"` / `"linux"` / `"windows"` so the
/// host can verify each per-OS spawn shape from a single
/// build. The per-OS arms are the same code that
/// `default_open_script_invoker` compiles into the host's
/// binary — the test simply makes the dispatch explicit so
/// every CI draft-build (draft-linux / draft-macos /
/// draft-windows) gets the right coverage.
fn default_open_script_invoker_for(target_os: &str) -> fn(&Path) -> std::process::Command {
    match target_os {
        "macos" => macos_open_command,
        "linux" => linux_xdg_open_command,
        "windows" => windows_cmd_start_command,
        _ => unsupported_command,
    }
}

/// Build the macOS `open -t <path>` Command. The `-t` flag
/// forces the default text editor so the user gets a
/// "look at your config" affordance consistent with the
/// wizard's promise.
fn macos_open_command(path: &Path) -> std::process::Command {
    let path_str = path.to_string_lossy().to_string();
    let mut cmd = std::process::Command::new("open");
    cmd.arg("-t").arg(&path_str);
    cmd
}

/// Build the Linux `xdg-open <path>` Command. `xdg-open` is
/// the freedesktop cross-DE default and the same binary the
/// frontend's reveal-script helper would shell out to.
fn linux_xdg_open_command(path: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("xdg-open");
    cmd.arg(path);
    cmd
}

/// Build the Windows `cmd /c start "" <path>` Command. The
/// empty `""` is required to disambiguate `start`'s title
/// argument from the path — `start <bare-path>` treats the
/// first quoted token as the title and the rest as the
/// path, so an empty title forces the second arg to be the
/// actual file.
fn windows_cmd_start_command(path: &Path) -> std::process::Command {
    let path_str = path.to_string_lossy().to_string();
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/c").arg("start").arg("").arg(&path_str);
    cmd
}

/// Fallback for hosts that aren't macOS / Linux / Windows
/// (e.g. FreeBSD, iOS). Returns an empty `Command` whose
/// `.spawn()` would fail; the production `let _ =` binding
/// discards the error so the Tauri command still returns the
/// path string the frontend can use.
fn unsupported_command(_path: &Path) -> std::process::Command {
    std::process::Command::new("true")
}

/// Shim that the test-side mock installs as the production
/// invoker. The mock closure lives behind a `Box<dyn FnMut>`
/// in `OPEN_SCRIPT_INVOKER`; this `fn` pointer is the
/// stable-address handle the production code calls when the
/// slot is `Some`. Always extracted as a `fn` (not a closure
/// type) so the `fn` return shape of `open_script_invoker`
/// is uniform — see Pitfall #119's "no clippy::await_holding_lock
/// / no std::sync::Mutex held across .await" notes.
fn test_invoker_shim(path: &Path) -> std::process::Command {
    let mut slot = OPEN_SCRIPT_INVOKER
        .lock()
        .expect("OPEN_SCRIPT_INVOKER mutex poisoned");
    if let Some(f) = slot.as_mut() {
        f(path)
    } else {
        // Defensive: if a test clears the slot mid-call (it
        // shouldn't — the RAII guard restores on drop), fall
        // through to the per-OS default. This branch is
        // unreachable in practice; the host-default Command
        // keeps the function total.
        drop(slot);
        (default_open_script_invoker())(path)
    }
}

/// Trait-object indirection that lets the test suite swap the
/// per-OS invoker out for a mock. `None` means "use the
/// default per-OS invoker" (production behaviour); tests set
/// this to a `Box<dyn FnMut>` that captures the path for that
/// test, and reset to `None` when their guard drops. Same
/// pattern §X-1 used for `INVOKE_INSTALL_SCRIPT`.
type OpenScriptInvoker = Box<dyn FnMut(&Path) -> std::process::Command + Send>;

static OPEN_SCRIPT_INVOKER: Mutex<Option<OpenScriptInvoker>> = Mutex::new(None);

/// Test-only helper: install a mock invoker for the duration of
/// the returned guard. The guard's `Drop` restores `None` (the
/// default invoker) so subsequent tests aren't poisoned by an
/// earlier test's mock. Returning a guard makes the swap
/// panic-safe: a test that returns early still resets the slot.
///
/// `#[allow(dead_code)]` because no committed test installs a
/// mock yet — the 3 current §X-2 tests assert the per-OS shape
/// via `default_open_script_invoker_for(...)` directly. The seam
/// is kept for the next test that needs to exercise the
/// production routing path (likely a `headless_launch.rs`
/// integration test in a follow-up).
#[cfg(test)]
#[allow(dead_code)]
fn set_open_script_invoker<F>(f: F) -> OpenScriptInvokerGuard
where
    F: FnMut(&Path) -> std::process::Command + Send + 'static,
{
    let mut slot = OPEN_SCRIPT_INVOKER
        .lock()
        .expect("OPEN_SCRIPT_INVOKER mutex poisoned");
    let prev = slot.replace(Box::new(f));
    drop(slot);
    OpenScriptInvokerGuard { prev }
}

/// RAII guard that resets `OPEN_SCRIPT_INVOKER` to `None` (the
/// default per-OS invoker) when dropped. Only constructed by
/// `set_open_script_invoker` from `#[cfg(test)]` code.
///
/// `#[allow(dead_code)]` mirrors the helper above — see the
/// rationale on `set_open_script_invoker`.
#[cfg(test)]
#[allow(dead_code)]
struct OpenScriptInvokerGuard {
    /// The previous invoker (if any). Saved so a future "stack"
    /// of nested installs can restore the prior mock; today we
    /// unconditionally reset to `None`, but the field keeps the
    /// drop body non-trivial and the door open for that
    /// future-proofing without an API change.
    prev: Option<OpenScriptInvoker>,
}

#[cfg(test)]
impl Drop for OpenScriptInvokerGuard {
    fn drop(&mut self) {
        let mut slot = OPEN_SCRIPT_INVOKER
            .lock()
            .expect("OPEN_SCRIPT_INVOKER mutex poisoned");
        *slot = self.prev.take();
    }
}

/// Tauri command: the "skip" path. Appends `"vps_collector"` to
/// `~/.trail/config.json`'s `pending_installs` array (idempotent).
/// The wizard's "do this later" button survives a restart because
/// the flag lives on disk.
#[tauri::command]
pub async fn mark_pending_install(
    app: tauri::AppHandle,
    collector_id: String,
) -> Result<(), String> {
    let p = user_config_path(&app);
    mark_pending_install_inner(&p, &collector_id).map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GitHubConfig, SshAuth, SummarizerConfig, TransportConfig, VoiceConfig};
    use std::io::Read;
    use std::path::PathBuf;

    /// Build a `Config` shaped like the Phase 6 §6.3 onboarding
    /// config-writer emits, with `transport` set to the Ssh variant
    /// the install plan needs. The transport host/port/user are
    /// the placeholder values the wizard uses by default; tests
    /// that care about the *rendered* plan override them.
    fn fixture_config() -> Config {
        Config {
            claude_sessions_paths: vec![PathBuf::from("/h/.claude/projects/work")],
            github: GitHubConfig {
                mode: "gh_cli".into(),
                host: "github.com".into(),
            },
            calendar: crate::config::CalendarSource::Ics {
                path: PathBuf::from("/h/Library/Calendars/work.calendar/Calendar.ics"),
            },
            calendar_ics: Some(PathBuf::from(
                "/h/Library/Calendars/work.calendar/Calendar.ics",
            )),
            voice: VoiceConfig {
                enabled: true,
                hotkey: "ctrl+shift+space".into(),
                transcriber: "whisper_cpp".into(),
                model: "base.en".into(),
                gpu_acceleration: true,
                gpu_fallback_logged: false,
            },
            review_time: "18:00".into(),
            summarizer: SummarizerConfig {
                model: "gpt-oss:20b".into(),
                model_provider: "local".into(),
                anonymization_strictness: "aggressive".into(),
                use_generic_categories: true,
                anonymization_rules: Vec::new(),
            },
            transport: TransportConfig::Ssh {
                host: "vm.pangolin-spica.ts.net".into(),
                port: 22,
                user: "vps_user".into(),
                auth: SshAuth::PublicKey {
                    path: PathBuf::from("~/.ssh/id_trail"),
                },
                remote_path: PathBuf::from("/home/vps_user/.trail/inbox/"),
            },
            raw_retention_days: 7,
            pending_installs: Vec::new(),
            // Phase 6 §6.3 extras — left at default for the install
            // tests (the install plan doesn't read them).
            github_repos: Vec::new(),
            calendar_paths: Vec::new(),
            voice_model: "base.en".into(),
            voice_language: "en".into(),
            summarizer_backend: "stub".into(),
            transport_method: "ssh".into(),
            ssh_key_path: Some(PathBuf::from("~/.ssh/id_trail")),
            browser_history: Default::default(),
            // ECD-5 — install tests don't exercise the
            // remote-calendar path, so leave the URL list
            // empty (the no-op path in the calendar
            // collector).
            remote_calendar_urls: Vec::new(),
        }
    }

    // ---- Test 1: render_install_plan shape -------------------------------

    /// The renderable plan must include enough metadata for the
    /// wizard's 3-option step to render: the binary path the auto
    /// path will write, the JSON path the show path will reveal, the
    /// cron line the wizard shows in its preview, and the post-install
    /// health output. The 3 wizard options are: auto (uses
    /// `apply_install_plan_localhost`), show (uses this rendered
    /// plan), skip (uses `mark_pending_install`).
    #[test]
    fn render_install_plan_includes_three_options_metadata() {
        let cfg = fixture_config();
        let plan = render_install_plan(&cfg, "vps_user").unwrap();
        assert!(plan.collector_bin_remote.ends_with("trail-collector"));
        assert!(plan.cron_line.contains("*/5 * * * *"));
        assert!(plan.health_output_line.starts_with("ok:"));
        assert!(plan.collector_json_remote.ends_with("collector.json"));
        // The SSH target round-trips through the plan so the
        // wizard can echo it in the "install will run as <user> at
        // <host>" line.
        assert_eq!(plan.ssh_user, "vps_user");
        assert_eq!(plan.ssh_host, "vm.pangolin-spica.ts.net");
        assert_eq!(plan.ssh_port, 22);
    }

    // ---- Test 2: install_vps_collector dry-run against mock-ssh-server --

    /// The load-bearing test: spawn the `mock-ssh-server` binary on
    /// an ephemeral port, call `install_vps_collector` with
    /// `dry_run: true`, and assert the mock server's inbox got the
    /// install JSON. This proves the auto-path code runs end-to-end
    /// without touching a real VPS — the host is headless.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_vps_collector_dry_run_succeeds_against_mock_ssh() {
        // 1. Pick an inbox dir for the mock server. Unique per run
        //    so parallel test invocations don't see each other's
        //    writes.
        let inbox = std::env::temp_dir().join(format!(
            "trail-mock-inbox-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::create_dir_all(&inbox).unwrap();
        let ready_file = std::env::temp_dir().join(format!(
            "trail-mock-ready-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        // Make sure no stale ready-file is sitting around.
        let _ = std::fs::remove_file(&ready_file);

        // 2. Resolve the workspace root so the test can find
        //    `target/debug/mock-ssh-server` (cargo test always
        //    sets `CARGO_MANIFEST_DIR` to the src-tauri crate).
        let manifest_dir =
            std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR set by cargo test");
        let workspace_root = PathBuf::from(manifest_dir).parent().unwrap().to_path_buf();
        let mock_bin = workspace_root
            .join("target")
            .join("debug")
            .join("mock-ssh-server");
        if !mock_bin.exists() {
            // The test's preflight gate must have built the
            // workspace; if the binary is missing, surface a clear
            // error so the next developer doesn't chase a flake.
            panic!(
                "mock-ssh-server binary not found at {}; run `cargo build -p mock-ssh-server` first",
                mock_bin.display()
            );
        }

        // 3. Spawn the mock server. `--port 0` requests an
        //    ephemeral port; the actual port is read back from
        //    `ready_file`.
        let inbox_str = inbox.to_string_lossy().to_string();
        let ready_file_str = ready_file.to_string_lossy().to_string();
        let mut child = std::process::Command::new(&mock_bin)
            .args([
                "--port",
                "0",
                "--inbox",
                &inbox_str,
                "--ready-file",
                &ready_file_str,
                "--one-shot",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn mock-ssh-server");

        // 4. Wait for the ready file (the server writes its bound
        //    port here on bind()).
        let mut port: Option<u16> = None;
        for _ in 0..50 {
            if ready_file.is_file() {
                let mut s = String::new();
                std::fs::File::open(&ready_file)
                    .unwrap()
                    .read_to_string(&mut s)
                    .unwrap();
                if let Some(line) = s.lines().next() {
                    if let Ok(p) = line.trim().parse::<u16>() {
                        port = Some(p);
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        let port = port.unwrap_or_else(|| {
            let _ = child.kill();
            panic!("mock-ssh-server did not write ready-file within 2.5s")
        });

        // 5. Now drive the Tauri command with the resolved port.
        //    The `LocalSshTarget::to_target` adapter round-trips
        //    the test-only port into the production
        //    `VpsInstallTarget` shape the Tauri command expects.
        let local = LocalSshTarget {
            port,
            collector_id: "vps_collector".to_string(),
        };
        let collector_id = local.collector_id.clone();
        let target = local.to_target("vps_user");
        // 2026-08-11 (PR #219) — `install_vps_collector`
        // now takes a `tauri::AppHandle` so the real-VPS
        // branch can call `user_config_path(&app)`. The
        // dry-run branch doesn't touch the config, so
        // pass any reasonable path (we point at a
        // non-existent temp path; `load_config` errors
        // out if the real-VPS branch runs, but the
        // dry-run branch never reaches it). Using a
        // pre-resolved `PathBuf` keeps the inner helper
        // runtime-agnostic — see the `install_vps_collector_inner`
        // doc for why.
        let fake_cfg_path = std::env::temp_dir().join(format!(
            "trail-fake-cfg-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let report = install_vps_collector_inner(&target, true, &fake_cfg_path)
            .await
            .expect("install_vps_collector dry-run should succeed");

        assert!(report.ok, "expected ok=true, got report: {report:?}");
        assert_eq!(report.dry_run_port, Some(port));

        // 6. The mock server writes a JSON file per connection.
        //    Wait briefly for the write to land, then assert.
        let mut files: Vec<PathBuf> = Vec::new();
        for _ in 0..20 {
            if let Ok(rd) = std::fs::read_dir(&inbox) {
                for entry in rd.flatten() {
                    files.push(entry.path());
                }
            }
            if !files.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(
            !files.is_empty(),
            "expected at least one install JSON in {}",
            inbox.display()
        );

        // 7. The first JSON file should have the expected shape:
        //    timestamp + collector_id + payload.
        let body = std::fs::read_to_string(&files[0]).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|e| {
            panic!("invalid JSON in {}: {e}\nbody: {body}", files[0].display())
        });
        assert_eq!(parsed["collector_id"], collector_id);
        assert!(parsed["timestamp"].is_string());
        assert!(parsed["payload"].is_string());

        // 8. Tear down. The `--one-shot` flag means the server
        //    already exited after the connection, but `wait()`
        //    reaps the zombie so the test doesn't leak.
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(&ready_file);
        let _ = std::fs::remove_dir_all(&inbox);
    }

    // ---- Test 3: mark_pending_install appends ----------------------------

    /// The skip path: write a fresh `config.json`, call
    /// `mark_pending_install_inner`, and assert the
    /// `pending_installs` array contains the install name.
    #[test]
    fn mark_pending_install_appends_to_config_json() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("config.json");
        let cfg = fixture_config();
        crate::onboarding::config_writer::write_config(&cfg, &dest).expect("write_config");

        mark_pending_install_inner(&dest, "vps_collector").expect("mark_pending_install");

        let loaded = crate::config::load_config(&dest).expect("load_config");
        assert!(
            loaded
                .pending_installs
                .contains(&"vps_collector".to_string()),
            "expected pending_installs to contain 'vps_collector', got: {:?}",
            loaded.pending_installs
        );
    }

    // ---- Test 4: mark_pending_install is idempotent ----------------------

    /// The skip path must be idempotent: calling it N times
    /// produces exactly one entry. A duplicate would surface as
    /// "install twice" in the wizard's "do this later" UI.
    #[test]
    fn mark_pending_install_is_idempotent() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let dest = tmp.path().join("config.json");
        let cfg = fixture_config();
        crate::onboarding::config_writer::write_config(&cfg, &dest).expect("write_config");

        mark_pending_install_inner(&dest, "vps_collector").expect("first call");
        mark_pending_install_inner(&dest, "vps_collector").expect("second call");
        mark_pending_install_inner(&dest, "vps_collector").expect("third call");

        let loaded = crate::config::load_config(&dest).expect("load_config");
        let count = loaded
            .pending_installs
            .iter()
            .filter(|x| x.as_str() == "vps_collector")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one 'vps_collector' entry, got: {:?}",
            loaded.pending_installs
        );
    }

    // ---- Test 5: real-VPS path invokes scripts/install-collector.sh ----

    /// Per-test recording + success-flag state for the mock
    /// bash invoker. The mock is installed via
    /// `set_install_script_invoker` with a closure that
    /// mutates an `Arc<Mutex<MockState>>` so the test's
    /// assertions and the production code's call see the
    /// same data without stepping on parallel tests'
    /// recordings.
    struct MockState {
        recorded: Option<(PathBuf, PathBuf)>,
        should_succeed: bool,
    }

    /// Process-wide serialisation mutex. The two real-VPS tests
    /// both mutate `$HOME` (so the production code resolves
    /// `collector_script_path()` to a tempdir under our
    /// control) and `INVOKE_INSTALL_SCRIPT` (so the production
    /// code's bash spawn is mocked). Under `cargo test`'s
    /// default parallel runner, two tests stomping on either
    /// of those would race; the simplest fix is to make the
    /// two tests mutually exclusive via this lock. A custom
    /// `Mutex<()>` (vs. `serial_test` or a thread-local) is
    /// the lightest weight option — no extra dev-dependency,
    /// no per-test attribute, and the lock is uncontended
    /// outside this test mod.
    static REAL_VPS_TEST_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> =
        std::sync::OnceLock::new();
    fn real_vps_test_lock() -> &'static tokio::sync::Mutex<()> {
        REAL_VPS_TEST_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    /// Spawn a child `sh -c "exit <N>"` and return its
    /// `Output` (with synthesised stdout/stderr). Used by the
    /// mock invoker to produce a real `ExitStatus` (the type
    /// has no public constructor on any platform) and a
    /// recognisable body for the wizard's toast / error
    /// message.
    fn run_synthetic_script(plan_path: &Path, config_path: &Path, exit_code: i32) -> Output {
        let mut child = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("exit {exit_code}"))
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn synthetic status");
        let status = child.wait().expect("wait synthetic status");
        Output {
            status,
            stdout: format!(
                "mock stdout (plan={}, cfg={})\n",
                plan_path.display(),
                config_path.display()
            )
            .into_bytes(),
            stderr: format!(
                "mock stderr (plan={}, cfg={})\n",
                plan_path.display(),
                config_path.display()
            )
            .into_bytes(),
        }
    }

    /// The real-VPS branch must invoke `bash scripts/install-collector.sh`
    /// with the rendered plan path + user config path handed in
    /// via `$TRAIL_INSTALL_PLAN` / `$TRAIL_INSTALL_CONFIG`. Mock
    /// the bash invoker so the test doesn't actually fork
    /// (`install-collector.sh` would try to ssh into a
    /// non-existent host and hang), and assert the recorded
    /// paths match the ones the production code derived from
    /// the live `config.json`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_vps_collector_invokes_shell_script_on_realvps_path() {
        // Hold the test-mod lock for the duration so a
        // parallel `install_vps_collector_surfaces_shell_errors`
        // doesn't stomp on `$HOME` or `INVOKE_INSTALL_SCRIPT`
        // mid-call. `tokio::sync::Mutex` (not std) so the guard
        // can safely span the `.await` below per
        // `clippy::await_holding_lock`.
        let _lock = real_vps_test_lock().lock().await;
        // 1. Build the per-test mock state. The closure
        //    captures an `Arc<Mutex<...>>` so the test
        //    driver can read back the recorded paths after
        //    `install_vps_collector_inner` returns.
        let state = std::sync::Arc::new(std::sync::Mutex::new(MockState {
            recorded: None,
            should_succeed: true,
        }));
        let state_for_closure = state.clone();
        let _guard = set_install_script_invoker(move |plan_path, config_path| {
            let mut guard = state_for_closure.lock().unwrap();
            guard.recorded = Some((plan_path.to_path_buf(), config_path.to_path_buf()));
            let should_succeed = guard.should_succeed;
            drop(guard);
            Ok(run_synthetic_script(
                plan_path,
                config_path,
                if should_succeed { 0 } else { 7 },
            ))
        });

        // 2. Write a real `config.json` into a tempdir so the
        //    production code can `load_config` it and render the
        //    plan.
        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg_path = tmp.path().join("config.json");
        let cfg = fixture_config();
        crate::onboarding::config_writer::write_config(&cfg, &cfg_path).expect("write_config");

        // 3. Redirect `HOME` to a tempdir so
        //    `collector_script_path()` writes the rendered plan
        //    somewhere under our control and we can compare
        //    against the recorded plan path. Setting HOME via
        //    `set_var` is `unsafe` on recent Rust editions
        //    (2024+) because of data-race concerns in
        //    multi-threaded programs; the test runs on the
        //    current thread only for the duration of the
        //    `install_vps_collector_inner` call (the other
        //    workers are blocked in the runtime), so the race
        //    window is closed for the assertion. Reset HOME on
        //    the way out so subsequent tests see the real value.
        let home_backup = std::env::var_os("HOME");
        let fake_home = tmp.path().join("home");
        std::fs::create_dir_all(&fake_home).expect("mkdir fake home");
        // SAFETY: see the comment above; the install helper is
        // called synchronously from the test thread and the
        // tokio workers are idle.
        unsafe { std::env::set_var("HOME", &fake_home) };
        let result = install_vps_collector_inner(
            &VpsInstallTarget {
                host: "vm.example.test".to_string(),
                port: 22,
                user: "vps_user".to_string(),
            },
            false,
            &cfg_path,
        )
        .await;
        match home_backup {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let report = result.expect("install_vps_collector real-VPS should succeed");

        // 4. The mock records `(plan_path, config_path)`. The
        //    plan path comes from `collector_script_path()` —
        //    i.e. `<HOME>/.trail/collector.json` under our
        //    overridden HOME. The config path is the
        //    `cfg_path` we wrote above.
        let recorded = state
            .lock()
            .unwrap()
            .recorded
            .clone()
            .expect("mock invoker was not called");
        let expected_plan_path = fake_home.join(".trail").join("collector.json");
        assert_eq!(
            recorded.0, expected_plan_path,
            "real-VPS path should hand the rendered plan path to install-collector.sh"
        );
        assert_eq!(
            recorded.1, cfg_path,
            "real-VPS path should hand the user config path to install-collector.sh"
        );
        assert!(
            report.ok,
            "expected ok=true when mock invoker returns success, got: {report:?}"
        );
        assert!(
            report.message.contains("mock stdout"),
            "expected the wizard's message to include the script's stdout, got: {:?}",
            report.message
        );
        assert_eq!(report.dry_run_port, None);
    }

    /// The real-VPS branch must surface a non-zero exit from
    /// `scripts/install-collector.sh` as a frontend-visible
    /// error. Mock the bash invoker to return an exit code of
    /// 7 with a recognisable stderr line, and assert the
    /// `Result::Err` body the Tauri command returns contains
    /// the synthetic stderr so the wizard can show the user
    /// what went wrong (refused SSH key, missing binary, etc.).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn install_vps_collector_surfaces_shell_errors() {
        let _lock = real_vps_test_lock().lock().await;
        let state = std::sync::Arc::new(std::sync::Mutex::new(MockState {
            recorded: None,
            should_succeed: false,
        }));
        let state_for_closure = state.clone();
        let _guard = set_install_script_invoker(move |plan_path, config_path| {
            let mut guard = state_for_closure.lock().unwrap();
            guard.recorded = Some((plan_path.to_path_buf(), config_path.to_path_buf()));
            let should_succeed = guard.should_succeed;
            drop(guard);
            Ok(run_synthetic_script(
                plan_path,
                config_path,
                if should_succeed { 0 } else { 7 },
            ))
        });

        let tmp = tempfile::tempdir().expect("tempdir");
        let cfg_path = tmp.path().join("config.json");
        let cfg = fixture_config();
        crate::onboarding::config_writer::write_config(&cfg, &cfg_path).expect("write_config");

        let home_backup = std::env::var_os("HOME");
        let fake_home = tmp.path().join("home");
        std::fs::create_dir_all(&fake_home).expect("mkdir fake home");
        // SAFETY: see the success-path test for the
        // multi-thread rationale; the inner helper is
        // synchronous from the test's perspective and the
        // tokio workers are idle while we hold the slot.
        unsafe { std::env::set_var("HOME", &fake_home) };
        let result = install_vps_collector_inner(
            &VpsInstallTarget {
                host: "vm.example.test".to_string(),
                port: 22,
                user: "vps_user".to_string(),
            },
            false,
            &cfg_path,
        )
        .await;
        match home_backup {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }

        let err = result.expect_err("real-VPS path should return Err when bash exits non-zero");
        // The error body must surface the script's exit code
        // and the synthesised stderr so the wizard can render
        // a useful "what went wrong" toast. Checking for the
        // exit-code line is the durable contract; the stderr
        // substring keeps the assertion close to the user-
        // visible behaviour.
        assert!(
            err.contains("exited with status 7"),
            "expected error to surface the script's exit code, got: {err:?}"
        );
        assert!(
            err.contains("mock stderr"),
            "expected error to surface the script's stderr, got: {err:?}"
        );
    }

    // ---- Test 7: open_collector_script uses `open -t` on macOS -----------
    //
    // The macOS arm of the per-OS dispatch must fire
    // `Command::new("open").args(["-t", &path])` so the user
    // gets a "look at your config in the default text editor"
    // affordance. This test is runnable on every host because
    // `default_open_script_invoker_for("macos")` is a pure
    // function that returns a `Command` for inspection — the
    // CI draft-macos build will exercise the real spawn in its
    // draft-build job; this host test verifies the
    // `Command::get_program()` + `Command::get_args()` shape
    // without forking.

    #[test]
    fn open_collector_script_uses_open_on_macos() {
        let path = std::path::Path::new("/tmp/.trail/collector.json");
        let cmd = default_open_script_invoker_for("macos")(path);
        assert_eq!(
            cmd.get_program(),
            "open",
            "macOS arm should use `open` as the program, got: {:?}",
            cmd.get_program()
        );
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(
            args.len(),
            2,
            "macOS arm should have exactly 2 args (`-t` + path), got: {:?}",
            args
        );
        assert_eq!(
            args[0], "-t",
            "macOS arm's first arg should be `-t` (force text editor), got: {:?}",
            args[0]
        );
        assert_eq!(
            args[1],
            path.as_os_str(),
            "macOS arm's second arg should be the collector.json path, got: {:?}",
            args[1]
        );
    }

    // ---- Test 8: open_collector_script uses `xdg-open` on Linux ----------
    //
    // The Linux arm must fire `Command::new("xdg-open").arg(&path)`
    // so the user gets a "look at your config" affordance on
    // every freedesktop-compliant desktop. This test runs on
    // every host (including the Linux CI draft-build) because
    // `default_open_script_invoker_for("linux")` is a pure
    // function returning a `Command` for inspection.

    #[test]
    fn open_collector_script_uses_xdg_open_on_linux() {
        let path = std::path::Path::new("/tmp/.trail/collector.json");
        let cmd = default_open_script_invoker_for("linux")(path);
        assert_eq!(
            cmd.get_program(),
            "xdg-open",
            "Linux arm should use `xdg-open` as the program, got: {:?}",
            cmd.get_program()
        );
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(
            args.len(),
            1,
            "Linux arm should have exactly 1 arg (the path), got: {:?}",
            args
        );
        assert_eq!(
            args[0],
            path.as_os_str(),
            "Linux arm's sole arg should be the collector.json path, got: {:?}",
            args[0]
        );
    }

    // ---- Test 9: open_collector_script uses `cmd /c start ""` on Windows -
    //
    // The Windows arm must fire
    // `Command::new("cmd").args(["/c", "start", "", &path])` so
    // the user gets a "look at your config" affordance on
    // Windows. The empty `""` is load-bearing: `start`'s first
    // quoted token is the title; without `""` the path is
    // interpreted as the title and `start` opens a new cmd
    // window with the wrong contents. This test runs on every
    // host because `default_open_script_invoker_for("windows")`
    // is a pure function returning a `Command` for inspection;
    // the Windows CI draft-build exercises the real spawn.

    #[test]
    fn open_collector_script_uses_cmd_start_on_windows() {
        let path = std::path::Path::new("/tmp/.trail/collector.json");
        let cmd = default_open_script_invoker_for("windows")(path);
        assert_eq!(
            cmd.get_program(),
            "cmd",
            "Windows arm should use `cmd` as the program, got: {:?}",
            cmd.get_program()
        );
        let args: Vec<&std::ffi::OsStr> = cmd.get_args().collect();
        assert_eq!(
            args.len(),
            4,
            "Windows arm should have exactly 4 args (`/c`, `start`, `\"\"`, path), got: {:?}",
            args
        );
        assert_eq!(
            args[0], "/c",
            "Windows arm's first arg should be `/c` (cmd's run-and-exit), got: {:?}",
            args[0]
        );
        assert_eq!(
            args[1], "start",
            "Windows arm's second arg should be `start`, got: {:?}",
            args[1]
        );
        assert_eq!(
            args[2], "",
            "Windows arm's third arg should be `\"\"` (empty title to disambiguate), got: {:?}",
            args[2]
        );
        assert_eq!(
            args[3],
            path.as_os_str(),
            "Windows arm's fourth arg should be the collector.json path, got: {:?}",
            args[3]
        );
    }
}
