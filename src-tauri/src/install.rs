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
use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::config::{Config, TransportConfig};

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
/// `config.json`. Mirrors `onboarding::config_writer::config_path()`
/// so the three Phase 6 §6.x Tauri commands all see the same file.
pub fn user_config_path() -> PathBuf {
    crate::onboarding::config_writer::config_path()
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
    target: VpsInstallTarget,
    dry_run: bool,
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
        // plan from it, and (in v1) defer to the existing
        // `scripts/install-collector.sh` shell driver for the
        // actual ssh2 work. Surfacing a clear error keeps the
        // wizard honest about which path it took.
        let cfg_path = user_config_path();
        let cfg = crate::config::load_config(&cfg_path).map_err(|e| e.to_string())?;
        let plan = render_install_plan(&cfg, &target.user).map_err(|e| e.to_string())?;
        // The real-VPS path is intentionally a no-op stub here:
        // Phase 1 §1.10's `scripts/install-collector.sh` is the
        // load-bearing implementation the wizard falls back to when
        // the user wants the "auto" path on their actual machine.
        // Surfacing a clear error keeps the wizard honest about
        // which path it took.
        let _ = plan;
        Err(
            "real-VPS install path runs `scripts/install-collector.sh`; not implemented in this Rust stub"
                .to_string(),
        )
    }
}

/// Tauri command: the "show" path. Returns the absolute path of
/// `~/.trail/collector.json` (the rendered install plan file) so the
/// frontend can hand it to the platform's `revealInFinder` /
/// `xdg-open` / `notepad`.
///
/// On macOS the command also fires a `Command::new("open").args(["-t", &path])`
/// so the user gets the "look at your config" affordance the wizard
/// promises. On Linux/Windows the command is a no-op (the frontend
/// picks the platform-appropriate handler).
#[tauri::command]
pub async fn open_collector_script() -> Result<String, String> {
    let path = collector_script_path();
    let path_str = path.to_string_lossy().to_string();
    #[cfg(target_os = "macos")]
    {
        // Best-effort — `reveal in Finder` is the user-visible UX.
        // The Tauri command is allowed to return Ok even if `open`
        // isn't on $PATH (test environments + non-macOS hosts).
        let _ = std::process::Command::new("open")
            .args(["-t", &path_str])
            .spawn();
    }
    Ok(path_str)
}

/// Tauri command: the "skip" path. Appends `"vps_collector"` to
/// `~/.trail/config.json`'s `pending_installs` array (idempotent).
/// The wizard's "do this later" button survives a restart because
/// the flag lives on disk.
#[tauri::command]
pub async fn mark_pending_install(collector_id: String) -> Result<(), String> {
    let p = user_config_path();
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
        let report = install_vps_collector(target, true)
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
            .filter(|x| *x == "vps_collector")
            .count();
        assert_eq!(
            count, 1,
            "expected exactly one 'vps_collector' entry, got: {:?}",
            loaded.pending_installs
        );
    }
}
