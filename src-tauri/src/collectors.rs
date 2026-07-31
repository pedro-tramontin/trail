//! Tauri-side collector orchestrator. Item 2-5 of the Phase 2 plan.
//!
//! Responsibilities:
//!
//! 1. Build a per-source [`CollectorOrchestrator`] from the laptop's
//!    [`Config`](crate::config::Config), picking which sources are enabled
//!    by default (GitHub iff `cfg.github.mode == "gh_cli"`, Claude sessions
//!    iff any path configured, calendar iff the configured `.ics` file
//!    exists).
//! 2. Expose `set_enabled`, `run_one`, and `info` for use by both the
//!    Settings UI (§2.6) and the `tokio-cron-scheduler` driven scheduler
//!    set up in `lib.rs::run`.
//! 3. Drive the bundled `trail-collector --collect <source> --laptop-config
//!    <path>` binary through `tokio::process::Command::output().await`. The
//!    collector itself stays synchronous; the orchestrator wraps it.
//!
//! State lives behind `Arc<Mutex<Inner>>` so cloning the orchestrator is
//! cheap (the `Manager` pattern) and the same state is shared across the
//! scheduler task, IPC commands, and the Settings UI.

use crate::config::Config;
use anyhow::{Context, Result};
use chrono::Utc;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;
use tracing::{info, warn};

/// Canonical ordering of the three collectors. Used by `info()` to return
/// them in this order regardless of insertion order in the underlying
/// `HashMap`, and by `set_enabled` to validate that the named source exists.
pub const CANONICAL_SOURCES: [&str; 3] = ["github", "claude_sessions", "calendar"];

/// Coarse status surfaced back to the frontend. Per-run success/failure is
/// carried in `last_exit_code`; `Status` reflects the toggle position + the
/// most recent run's outcome so the UI can badge persistent failures.
///
/// Public API surface item — currently constructed by tests only and
/// consumed by the Settings UI (§2.6) via `serde(rename_all = "snake_case")`.
/// Allowed as `dead_code` so adding the type doesn't require a §2.6
/// consumer to be merged first (avoids a parallel branch dependency).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollectorStatus {
    Enabled,
    Disabled,
    Error,
}

/// Serialised view of one collector for the Settings UI. Field names mirror
/// the Phase 2 §2.6 Svelte 5 component (`run-now`, `last-run`, etc.).
#[derive(Debug, Clone, Serialize)]
pub struct CollectorInfo {
    pub source: String,
    pub enabled: bool,
    pub schedule: String,
    pub last_run_at: Option<chrono::DateTime<Utc>>,
    pub last_exit_code: Option<i32>,
    pub last_error: Option<String>,
}

/// Cheap-to-clone handle to the orchestrator state.
#[derive(Clone)]
pub struct CollectorOrchestrator {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    config_path: PathBuf,
    collector_bin: PathBuf,
    toggles: std::collections::HashMap<String, CollectorToggle>,
}

#[derive(Clone)]
struct CollectorToggle {
    enabled: bool,
    schedule: String,
    last_run_at: Option<chrono::DateTime<Utc>>,
    last_exit_code: Option<i32>,
    last_error: Option<String>,
}

impl CollectorOrchestrator {
    /// Construct an orchestrator with default-enable rules derived from
    /// `cfg`. `config_path` and `collector_bin` are stored for later
    /// `run_one` invocations and need not point to a real file at
    /// construction time (the constructor doesn't read either).
    pub fn new(config_path: PathBuf, collector_bin: PathBuf, cfg: &Config) -> Self {
        let mk = |enabled: bool| CollectorToggle {
            enabled,
            schedule: "@hourly".into(),
            last_run_at: None,
            last_exit_code: None,
            last_error: None,
        };
        let mut toggles = std::collections::HashMap::new();
        toggles.insert("github".into(), mk(cfg.github.mode == "gh_cli"));
        toggles.insert(
            "claude_sessions".into(),
            mk(!cfg.claude_sessions_paths.is_empty()),
        );
        toggles.insert("calendar".into(), mk(cfg.calendar_ics.exists()));
        Self {
            inner: Arc::new(Mutex::new(Inner {
                config_path,
                collector_bin,
                toggles,
            })),
        }
    }

    /// Snapshot all collectors in canonical order. Always returns the three
    /// known sources even if the underlying toggles map has been mutated by
    /// future code paths.
    pub async fn info(&self) -> Vec<CollectorInfo> {
        let inner = self.inner.lock().await;
        CANONICAL_SOURCES
            .iter()
            .map(|name| {
                let t = inner
                    .toggles
                    .get(*name)
                    .cloned()
                    .unwrap_or(CollectorToggle {
                        enabled: false,
                        schedule: "@hourly".into(),
                        last_run_at: None,
                        last_exit_code: None,
                        last_error: None,
                    });
                CollectorInfo {
                    source: (*name).to_string(),
                    enabled: t.enabled,
                    schedule: t.schedule,
                    last_run_at: t.last_run_at,
                    last_exit_code: t.last_exit_code,
                    last_error: t.last_error,
                }
            })
            .collect()
    }

    /// Flip a source's enabled toggle in memory. Returns an error on
    /// unknown source so the IPC command surfaces a clean string to the UI.
    pub async fn set_enabled(&self, source: &str, enabled: bool) -> Result<()> {
        let mut inner = self.inner.lock().await;
        let t = inner
            .toggles
            .get_mut(source)
            .ok_or_else(|| anyhow::anyhow!("unknown source: {source}"))?;
        t.enabled = enabled;
        Ok(())
    }

    /// Spawn the bundled collector for one source synchronously and return
    /// its exit code. Side effects: writes a temporary laptop-config file
    /// (deleted on return), executes `collector_bin --collect <source>
    /// --laptop-config <path>`, records the last-run timestamp + exit code
    /// + stderr excerpt into the toggle state.
    ///
    /// The synthetic `--config /dev/null` is supplied because the collector
    /// CLI requires `--config <path>` even when unused for `--collect`.
    pub async fn run_one(&self, source: &str) -> Result<i32> {
        // Reload config from disk so config edits made between Tauri starts
        // (or via the Settings wizard) are picked up on the next tick.
        let (config_path, collector_bin) = {
            let inner = self.inner.lock().await;
            (inner.config_path.clone(), inner.collector_bin.clone())
        };
        let cfg = crate::config::load_config(&config_path).context("loading config")?;
        let (laptop_cfg, schema_filename) = build_laptop_cfg(source, &cfg)?;
        // Stamp in the resolved schema path so the supervisor (which reads
        // it from the laptop config) finds the bundled per-source schema.
        let resources_dir = find_resources_dir()?;
        let schema_path = resources_dir.join(schema_filename);
        if !schema_path.exists() {
            anyhow::bail!(
                "schema not bundled: {} (run `cargo build` first to copy schemas via the build.rs)",
                schema_path.display()
            );
        }
        let mut laptop_cfg = laptop_cfg;
        laptop_cfg.schema_path = schema_path;

        let tmp = std::env::temp_dir().join(format!(
            "trail-collect-{source}-{}-{}.json",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        std::fs::write(&tmp, serde_json::to_string_pretty(&laptop_cfg)?)
            .with_context(|| format!("writing temp laptop config {}", tmp.display()))?;

        let output = Command::new(&collector_bin)
            .args([
                "--config",
                "/dev/null",
                "--collect",
                source,
                "--laptop-config",
                tmp.to_str().unwrap(),
            ])
            .output()
            .await
            .with_context(|| format!("spawning {collector_bin:?}"));
        let _ = std::fs::remove_file(&tmp);
        let output = output?;

        let code = output.status.code().unwrap_or(-1);
        let stderr = if code != 0 {
            Some(String::from_utf8_lossy(&output.stderr).into_owned())
        } else {
            // Even on success a non-empty stderr is worth surfacing in `last_error`
            // so the UI can display "ran fine but printed warnings". Treat empty
            // stderr as "no error" so the UI doesn't get a blank error row.
            let s = String::from_utf8_lossy(&output.stderr).into_owned();
            if s.trim().is_empty() {
                None
            } else {
                Some(s)
            }
        };

        {
            let mut inner = self.inner.lock().await;
            if let Some(t) = inner.toggles.get_mut(source) {
                t.last_run_at = Some(Utc::now());
                t.last_exit_code = Some(code);
                t.last_error = stderr;
            }
        }

        if code != 0 {
            warn!(source, code, "collector exited non-zero");
        } else {
            info!(source, code, "collector exited 0");
        }
        Ok(code)
    }
}

/// Resolve the directory where the build.rs copies per-source schemas.
/// Compile-time anchor: `src-tauri/`. Fails loudly if the dir isn't there
/// — a release build that lacks the schemas would be a critical regression.
fn find_resources_dir() -> Result<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    if p.exists() {
        Ok(p)
    } else {
        anyhow::bail!("src-tauri/resources/ not found at {}", p.display())
    }
}

/// Mirrors `trail_collector::collectors::CollectorLaptopConfig` shape so we
/// can serialise a JSON file the bundled binary can parse verbatim. Kept as
/// a local struct so the Tauri side can evolve independently of the
/// collector (and so we don't pull `trail-collector` in as a runtime
/// dep — it's an external process).
#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct LaptopCfg {
    source: String,
    github: GithubLaptopCfg,
    claude_sessions_paths: Vec<PathBuf>,
    calendar_ics: PathBuf,
    raw_root: PathBuf,
    schema_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
struct GithubLaptopCfg {
    mode: String,
    host: String,
    enabled: bool,
}

/// Build the per-source slice of `LaptopCfg` plus the canonical schema
/// filename (used by `run_one` to verify the schema is bundled before
/// spawning the binary).
fn build_laptop_cfg(source: &str, cfg: &Config) -> Result<(LaptopCfg, &'static str)> {
    let gh = GithubLaptopCfg {
        mode: cfg.github.mode.clone(),
        host: cfg.github.host.clone(),
        enabled: source == "github",
    };
    let raw_root = PathBuf::from(
        std::env::var("HOME").context("HOME not set; required to compute ~/.trail/raw")?,
    )
    .join(".trail")
    .join("raw");
    let sources: &[(&str, Vec<PathBuf>, &'static str)] = &[
        ("github", vec![], "github.schema.json"),
        (
            "claude_sessions",
            cfg.claude_sessions_paths.clone(),
            "claude_sessions.schema.json",
        ),
        ("calendar", vec![], "calendar.schema.json"),
    ];
    for (name, paths, schema) in sources {
        if *name == source {
            let laptop = LaptopCfg {
                source: (*name).to_string(),
                github: gh,
                claude_sessions_paths: paths.clone(),
                calendar_ics: cfg.calendar_ics.clone(),
                raw_root,
                schema_path: PathBuf::new(),
            };
            return Ok((laptop, *schema));
        }
    }
    anyhow::bail!("unknown source: {source}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        Config, GitHubConfig, SshAuth, SummarizerConfig, TransportConfig, VoiceConfig,
    };
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;
    use tempfile::TempDir;

    /// Build a fully-populated `Config` rooted at `tmp` so the existence
    /// check on `calendar_ics` succeeds and `claude_sessions_paths` is
    /// non-empty — matching the spec's "all three collectors enabled in
    /// canonical order" test arrangement.
    fn make_cfg(tmp: &Path) -> Config {
        Config {
            claude_sessions_paths: vec![tmp.join("claude/work")],
            github: GitHubConfig {
                mode: "gh_cli".into(),
                host: "github.com".into(),
            },
            calendar_ics: tmp.join("cal.ics"),
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
            },
            transport: TransportConfig::Ssh {
                host: "x".into(),
                port: 22,
                user: "pedro".into(),
                auth: SshAuth::PublicKey {
                    path: PathBuf::from("/k"),
                },
                remote_path: PathBuf::from("/r/"),
            },
            raw_retention_days: 7,
            pending_installs: vec![],
        }
    }

    /// Materialise a fake `trail-collector` binary on disk that just runs
    /// `bin_script` as a shell program. Returns the orchestrator + the
    /// tempdir (caller holds the tempdir alive for the duration of the
    /// test) + the path of the fake binary. The orchestrator is built
    /// with the `config_path` pointing at a real `config.json` written
    /// into the tempdir so `run_one` can reload it.
    fn make_orch(bin_script: &str) -> (TempDir, PathBuf, CollectorOrchestrator) {
        let tmp = TempDir::new().expect("tempdir");
        // Make `cfg.calendar_ics.exists()` true so calendar is enabled.
        std::fs::write(
            tmp.path().join("cal.ics"),
            "BEGIN:VCALENDAR\nEND:VCALENDAR\n",
        )
        .expect("write cal.ics");
        let cfg = make_cfg(tmp.path());
        // Write a real `config.json` so `run_one`'s `load_config` succeeds.
        // The Schema is the same on disk as the in-memory `Config`, so the
        // canonical-source fan-out below produces the same output struct.
        let config_path = tmp.path().join("config.json");
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&cfg).expect("serialise cfg"),
        )
        .expect("write config.json");
        let bin = tmp.path().join("fake-trail-collector");
        std::fs::write(&bin, bin_script).expect("write bin script");
        std::fs::set_permissions(&bin, PermissionsExt::from_mode(0o755)).expect("chmod bin");
        let orch = CollectorOrchestrator::new(config_path, bin.clone(), &cfg);
        (tmp, bin, orch)
    }

    /// Test 1 (spec): `set_enabled` flips the in-memory toggle and the
    /// change is observable through `info()`.
    #[tokio::test]
    async fn set_enabled_flips_state() {
        let (_tmp, _bin, orch) = make_orch("#!/bin/sh\nexit 0\n");
        // All three start enabled (cal.ics file exists, github mode =
        // gh_cli, claude path non-empty).
        let before = orch.info().await;
        assert!(
            before.iter().all(|c| c.enabled),
            "all collectors start enabled"
        );

        orch.set_enabled("github", false).await.unwrap();
        let after = orch.info().await;
        let gh = after.iter().find(|c| c.source == "github").unwrap();
        assert!(!gh.enabled, "github should now be disabled");

        orch.set_enabled("github", true).await.unwrap();
        let after = orch.info().await;
        let gh = after.iter().find(|c| c.source == "github").unwrap();
        assert!(gh.enabled, "github should now be re-enabled");
    }

    /// Test 2 (spec): `run_one` succeeds (exit code 0) when the spawned
    /// collector binary exits cleanly. The bundled schema check passes
    /// because `find_resources_dir()` resolves to `src-tauri/resources/`,
    /// which contains all three per-source schemas.
    #[tokio::test]
    async fn run_one_succeeds_against_bundled_schema() {
        let (_tmp, _bin, orch) = make_orch("#!/bin/sh\nexit 0\n");
        let code = orch.run_one("calendar").await.expect("run_one");
        assert_eq!(code, 0, "calendar collector should exit 0");
        let info = orch.info().await;
        let cal = info
            .iter()
            .find(|c| c.source == "calendar")
            .unwrap()
            .clone();
        assert_eq!(cal.last_exit_code, Some(0));
        assert!(cal.last_run_at.is_some());
        assert!(
            cal.last_error.is_none(),
            "expected no error on success: {:?}",
            cal.last_error
        );
    }

    /// Test 3 (spec): `run_one` returns non-zero when the spawned
    /// collector binary exits non-zero (simulating a schema mismatch /
    /// upstream failure). The orchestrator records the exit code and the
    /// captured stderr into the toggle state.
    #[tokio::test]
    async fn run_one_returns_nonzero_on_schema_mismatch() {
        let (_tmp, _bin, orch) =
            make_orch("#!/bin/sh\necho 'schema validation failed for foo' >&2\nexit 1\n");
        let code = orch.run_one("github").await.expect("run_one");
        assert_ne!(code, 0, "github collector should exit non-zero");
        let info = orch.info().await;
        let gh = info.iter().find(|c| c.source == "github").unwrap().clone();
        assert_eq!(gh.last_exit_code, Some(code));
        assert!(gh.last_run_at.is_some());
        assert!(
            gh.last_error
                .as_deref()
                .map(|s| s.contains("schema"))
                .unwrap_or(false),
            "expected schema-related stderr in last_error, got {:?}",
            gh.last_error
        );
    }

    /// Test 4 (spec): `info()` returns the three sources in canonical
    /// order regardless of `HashMap` insertion order.
    #[tokio::test]
    async fn info_returns_canonical_order() {
        let (_tmp, _bin, orch) = make_orch("#!/bin/sh\nexit 0\n");
        let names: Vec<String> = orch.info().await.into_iter().map(|c| c.source).collect();
        assert_eq!(
            names,
            vec![
                "github".to_string(),
                "claude_sessions".to_string(),
                "calendar".to_string()
            ],
            "info() must return collectors in canonical order"
        );
    }
}
