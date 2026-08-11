use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::anonymizer::AnonymizationRule;

/// Top-level laptop config, loaded from `~/.trail/config.json`.
/// Mirrors the schema frozen in the master plan. Every field is required
/// in v1 — there are no serde defaults because a missing required field
/// is a config error, not a silently-filled one.
///
/// Phase 6 §6.3 added the trailing seven fields (all `#[serde(default)]`)
/// so v1 config blobs written before the Phase C config-writer still
/// deserialize cleanly. They carry the LLM-driven onboarding answers
/// (github repos, calendar paths, voice language, summarizer backend
/// string, transport method string, ssh key path) that don't have a
/// natural home in the frozen v1 schema.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub claude_sessions_paths: Vec<PathBuf>,
    pub github: GitHubConfig,
    /// 2026-08-11 — calendar data source choice. The legacy
    /// `calendar_ics: PathBuf` field is removed from the *type* but
    /// the on-disk JSON can still carry it: `load_config` reads the
    /// legacy shape and migrates it to `calendar = { kind: "ics",
    /// path: ... }` before returning. The migration shim lives
    /// in `load_config` below.
    ///
    /// `#[serde(default)]` keeps backwards-compat with on-disk
    /// configs that haven't been touched since the PR landed —
    /// serde will default the field to
    /// `CalendarSource::default()` (an empty `Ics { path }`) when
    /// the JSON omits the key, and `load_config`'s shim then
    /// overwrites with the legacy `calendar_ics` value if the
    /// shim is also present.
    #[serde(default)]
    pub calendar: CalendarSource,
    /// Legacy `calendar_ics: PathBuf` shim. Optional in serde so
    /// new configs don't write it. Read by `load_config` on
    /// legacy blobs and remapped to `calendar` (Ics { path }).
    /// Never written by `config_writer.rs` — the new field
    /// `calendar` is the canonical source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_ics: Option<PathBuf>,
    pub voice: VoiceConfig,
    pub review_time: String,
    pub summarizer: SummarizerConfig,
    pub transport: TransportConfig,
    pub raw_retention_days: u32,
    pub pending_installs: Vec<String>,

    // --- Phase 6 §6.3 onboarding extras (all serde default so older
    //     configs load cleanly) ---
    /// `github` collector repos (`owner/repo` slugs). Populated by the
    /// Phase C config-writer from the LLM's answers; older v1 configs
    /// have this empty.
    #[serde(default)]
    pub github_repos: Vec<String>,
    /// Calendar `.ics` file paths the calendar_ics collector should
    /// watch. Phase C maps the first one to [`Config::calendar`]'s
    /// `Ics { path }` variant and carries the rest here.
    #[serde(default)]
    pub calendar_paths: Vec<String>,
    /// Whisper model id (e.g. `"base.en"`). Mirrors
    /// [`VoiceConfig::model`] so the wizard can read it without
    /// following the typed-struct pointer.
    #[serde(default)]
    pub voice_model: String,
    /// BCP-47 primary subtag for voice transcription (e.g. `"en"`).
    #[serde(default = "default_voice_language")]
    pub voice_language: String,
    /// `"ollama" | "stub"` — the LLM's `summarizer.backend` answer.
    #[serde(default = "default_summarizer_backend")]
    pub summarizer_backend: String,
    /// `"tailscale" | "ssh"` — the LLM's `transport.method` answer.
    /// Independent of the typed [`TransportConfig`] (which carries
    /// the SSH skeleton).
    #[serde(default = "default_transport_method")]
    pub transport_method: String,
    /// SSH key path. `Some` after Phase C emits the config
    /// post-keypair-generation; `None` when the keychain still holds
    /// the password slot.
    #[serde(default)]
    pub ssh_key_path: Option<PathBuf>,
    /// 2026-08-11 (PR #221, plan §browser-history): browser-history
    /// configuration. Carries the user's browser pick (set by the
    /// wizard's StepAsk row) intersected with the scanner's
    /// `Available` set. Empty `browsers` = "no browsing captured
    /// today" — the collector subprocess still runs and emits an
    /// empty envelope so the daily review prompt can render
    /// "no browsing captured today".
    ///
    /// `#[serde(default)]` keeps backwards-compat with on-disk
    /// configs that haven't been touched since PR #219.
    #[serde(default)]
    pub browser_history: BrowserHistoryConfig,
}

/// 2026-08-11 (PR #221): what the laptop-side supervisor passes
/// to the browser-history collector subprocess. Mirrors the
/// calendar `CalendarSource` shape — the wizard's
/// `answers.browser_history` is intersected with the scanner's
/// `Available` set during config-write, then this typed struct
/// is what the supervisor reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BrowserHistoryConfig {
    /// Browsers the user picked AND the scanner confirmed
    /// available. Drives which per-browser reader runs in the
    /// collector subprocess. Empty = nothing to capture (the
    /// subprocess still emits an empty envelope).
    pub browsers: Vec<BrowserKind>,
    /// The supervisor fills these from the scanner's
    /// `evidence.path` field for each picked browser. The
    /// collector subprocess opens each path read-only via the
    /// copy-to-temp + read-only-open pattern (plan §D2).
    #[serde(default)]
    pub db_paths: Vec<BrowserDbPath>,
}

/// 2026-08-11 (PR #221): the laptop-side `BrowserKind` enum
/// mirrors the collector subprocess's `Browser` (in
/// `crates/trail-collector/src/collectors/synth_browser_history`)
/// and is serialised as the lowercase string form for the on-disk
/// JSON. The scanner in `src-tauri/src/onboarding/scan.rs` emits
/// `BrowserKind`; the config-writer in
/// `src-tauri/src/onboarding/config_writer.rs` reads it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BrowserKind {
    Chrome,
    Brave,
    Opera,
    Firefox,
    Safari,
}

impl BrowserKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Brave => "brave",
            Self::Opera => "opera",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BrowserDbPath {
    pub browser: BrowserKind,
    pub path: PathBuf,
    pub profile: String,
}

/// The calendar collector's data source. `Ics` is the legacy
/// `.ics`-file path (always available); `EventKit` is macOS-only
/// and tells the laptop-side collector to read from Apple Calendar
/// via the `EventKit.framework`. A user who picks `EventKit` on
/// Linux (or any non-macOS target) is rejected by the Tauri
/// side's `Config::validate` before the collector subprocess is
/// ever spawned — the collector's `run` defensive `unreachable!`
/// is the last-line guard.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CalendarSource {
    /// Apple Calendar.app via the `EventKit.framework`.
    /// macOS only. The collector reads the user's calendars
    /// directly; a TCC prompt is required once per install
    /// (full-calendar-access on macOS 14+).
    EventKit {
        /// Calendars to include, by their `EKCalendar.title`.
        /// `None` = "all calendars the user can see", which is
        /// what most users want and is the v1 default.
        calendars: Option<Vec<String>>,
    },
    /// Static `.ics` file path. Used on Linux (Evolution, etc.)
    /// and as the macOS fallback when the user can't grant TCC.
    Ics { path: PathBuf },
}

impl Default for CalendarSource {
    /// The pre-2026-08-11 default for fresh configs: an empty
    /// `.ics` path. The wizard's Ask step's first action is to
    /// overwrite this with the user's real choice (EventKit on
    /// macOS, Ics on Linux).
    fn default() -> Self {
        Self::Ics {
            path: PathBuf::new(),
        }
    }
}

fn default_voice_language() -> String {
    "en".to_string()
}

fn default_summarizer_backend() -> String {
    "stub".to_string()
}

fn default_transport_method() -> String {
    "ssh".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GitHubConfig {
    /// v1 only: "gh_cli". Reserved for v2 direct API.
    pub mode: String,
    pub host: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub hotkey: String,
    /// v1 only: "whisper_cpp". Reserved for v2 cloud STT.
    pub transcriber: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SummarizerConfig {
    pub model: String,
    /// "local" | "ollama_cloud"
    pub model_provider: String,
    /// "aggressive" | "moderate" | "off" — defaults to "aggressive" if
    /// absent in the config file, so older `config.json` blobs that omit
    /// this field still load.
    #[serde(default = "default_anonymization_strictness")]
    pub anonymization_strictness: String,
    pub use_generic_categories: bool,
    /// Per-user anonymization rules (literal substring → placeholder).
    /// Replaces explicit names ("ACME Corp" → "[COMPANY]") before any
    /// built-in regex scrubber runs. Defaults to empty so older configs
    /// without this field still load.
    #[serde(default)]
    pub anonymization_rules: Vec<AnonymizationRule>,
}

/// Serde helper for the `anonymization_strictness` default. Returns
/// `"aggressive"` so an absent field is indistinguishable from
/// `"aggressive"` in the parsed config (rather than the
/// empty-string that `#[serde(default)]` would have produced on
/// `String`).
fn default_anonymization_strictness() -> String {
    "aggressive".to_string()
}

/// Transport is an open-ended enum: v1 ships only `Ssh`, but
/// `#[serde(tag = "type", rename_all = "snake_case")]` means v2 can
/// add `Https` / `S3` / `Database` variants without breaking the
/// v1 JSON shape (each variant emits its own `"type"` discriminator).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TransportConfig {
    Ssh {
        host: String,
        port: u16,
        user: String,
        auth: SshAuth,
        remote_path: PathBuf,
    },
}

/// SSH auth method. `PublicKey` is the v1 default (keypair gen from
/// §1.3, stored in macOS Keychain). `Password` is reserved for v2 —
/// the v1 onboarding wizard only writes the public_key variant; the
/// Password variant exists in the type system so deserialising a v2
/// config doesn't reject on v1.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "auth", rename_all = "snake_case")]
pub enum SshAuth {
    PublicKey { path: PathBuf },
    Password { env_var: String },
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config file not found at {0}")]
    NotFound(PathBuf),
    #[error("invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

pub fn load_config(path: &Path) -> Result<Config, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::NotFound(path.to_path_buf()));
    }
    let contents = std::fs::read_to_string(path)?;
    // 2026-08-11 — migration shim. The on-disk JSON may carry the
    // legacy `calendar_ics: PathBuf` field (a flat string) instead
    // of the new `calendar: { kind: "ics", path: ... }` enum. We
    // accept both: serde's deserialise into `Config` will leave
    // `calendar` defaulted (empty `Ics { path: "" }`) and the
    // `calendar_ics` shim field populated with the legacy string.
    // After deserialise, we read the shim, override `calendar` if
    // the shim is set, and return. New configs (the wizard
    // rewrites the file via `config_writer.rs`) populate `calendar`
    // directly; the shim is `skip_serializing_if = "Option::is_none"`
    // so it's never re-written. The migration runs in
    // O(file-size) per load — no real cost.
    let mut config: Config = serde_json::from_str(&contents)?;
    if let Some(legacy_path) = config.calendar_ics.take() {
        // `ref path` borrows the inner PathBuf instead of moving
        // it, so the `matches!` guard below doesn't trigger
        // E0382 (partial move) when we also need to read
        // `config.calendar` again on the `Ok(config)` line.
        if matches!(&config.calendar, CalendarSource::Ics { ref path } if path.as_os_str().is_empty())
        {
            config.calendar = CalendarSource::Ics { path: legacy_path };
        }
        // else: the new `calendar` field is already populated with
        // a real value (e.g. an EventKit choice), and the legacy
        // `calendar_ics` field is just leftover from a pre-PR
        // wizard that didn't strip it. Drop it without overwriting
        // the new field.
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp_config(json: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    #[test]
    fn load_valid_ssh_config() {
        let json = r#"{
            "claude_sessions_paths": ["~/.claude/projects/work"],
            "github": {"mode": "gh_cli", "host": "github.com"},
            "calendar_ics": "~/Library/Calendars/work.ics",
            "voice": {"enabled": true, "hotkey": "ctrl+shift+space", "transcriber": "whisper_cpp", "model": "base.en"},
            "review_time": "18:00",
            "summarizer": {"model": "gpt-oss:20b", "model_provider": "local", "anonymization_strictness": "aggressive", "use_generic_categories": true},
            "transport": {"type": "ssh", "host": "vm.example.com", "port": 22, "user": "pedro", "auth": {"auth": "public_key", "path": "~/.ssh/id_ed25519"}, "remote_path": "~/.hermes/plans/career-coaching-pedro/daily"},
            "raw_retention_days": 7,
            "pending_installs": []
        }"#;
        let f = write_temp_config(json);
        let cfg = load_config(f.path()).unwrap();
        assert_eq!(cfg.review_time, "18:00");
        assert_eq!(cfg.raw_retention_days, 7);
        // 2026-08-11 — the legacy `calendar_ics` field is migrated
        // to `CalendarSource::Ics { path }` by `load_config`.
        assert_eq!(
            cfg.calendar,
            CalendarSource::Ics {
                path: PathBuf::from("~/Library/Calendars/work.ics"),
            }
        );
        assert!(cfg.calendar_ics.is_none(), "shim field is cleared post-migration");
        match &cfg.transport {
            TransportConfig::Ssh {
                host,
                port,
                user,
                auth,
                ..
            } => {
                assert_eq!(host, "vm.example.com");
                assert_eq!(*port, 22);
                assert_eq!(user, "pedro");
                match auth {
                    SshAuth::PublicKey { path } => {
                        assert_eq!(path.to_string_lossy(), "~/.ssh/id_ed25519");
                    }
                    _ => panic!("expected PublicKey auth"),
                }
            }
        }
    }

    #[test]
    fn missing_file_returns_not_found() {
        let result = load_config(Path::new("/nonexistent/path/config.json"));
        assert!(matches!(result, Err(ConfigError::NotFound(_))));
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let f = write_temp_config("{ not valid json");
        let result = load_config(f.path());
        assert!(matches!(result, Err(ConfigError::InvalidJson(_))));
    }

    #[test]
    fn missing_required_field_returns_parse_error() {
        // Missing the `transport` field.
        let json = r#"{"claude_sessions_paths":[],"github":{"mode":"gh_cli","host":"x"},"calendar_ics":"x","voice":{"enabled":false,"hotkey":"x","transcriber":"x","model":"x"},"review_time":"x","summarizer":{"model":"x","model_provider":"x","anonymization_strictness":"x","use_generic_categories":false},"raw_retention_days":1,"pending_installs":[]}"#;
        let f = write_temp_config(json);
        let result = load_config(f.path());
        assert!(matches!(result, Err(ConfigError::InvalidJson(_))));
    }

    /// Regression: the pre-fix `#[serde(default)]` on
    /// `anonymization_strictness` made an absent field default to
    /// the empty string, which the `AnonymizationStrictness` parser
    /// would then map to `Aggressive` — silently OK for production
    /// but the loaded config still showed `""` instead of
    /// `"aggressive"`. The fix uses
    /// `#[serde(default = "default_anonymization_strictness")]`
    /// so an absent field loads as `"aggressive"`.
    #[test]
    fn anonymization_strictness_defaults_to_aggressive() {
        // `anonymization_strictness` is omitted from the JSON.
        let json = r#"{
            "claude_sessions_paths": [],
            "github": {"mode": "gh_cli", "host": "x"},
            "calendar_ics": "x",
            "voice": {"enabled": false, "hotkey": "x", "transcriber": "x", "model": "x"},
            "review_time": "18:00",
            "summarizer": {"model": "x", "model_provider": "local", "use_generic_categories": false},
            "transport": {"type": "ssh", "host": "x", "port": 22, "user": "u", "auth": {"auth": "password", "env_var": "X"}, "remote_path": "/tmp/x"},
            "raw_retention_days": 7,
            "pending_installs": []
        }"#;
        let f = write_temp_config(json);
        let cfg = load_config(f.path()).unwrap();
        assert_eq!(cfg.summarizer.anonymization_strictness, "aggressive");
    }

    #[test]
    fn password_auth_variant_parses() {
        let json = r#"{
            "claude_sessions_paths": [],
            "github": {"mode": "gh_cli", "host": "x"},
            "calendar_ics": "x",
            "voice": {"enabled": false, "hotkey": "x", "transcriber": "x", "model": "x"},
            "review_time": "18:00",
            "summarizer": {"model": "x", "model_provider": "local", "anonymization_strictness": "aggressive", "use_generic_categories": false},
            "transport": {"type": "ssh", "host": "x", "port": 22, "user": "u", "auth": {"auth": "password", "env_var": "SSH_PASSWORD"}, "remote_path": "/tmp/x"},
            "raw_retention_days": 7,
            "pending_installs": []
        }"#;
        let f = write_temp_config(json);
        let cfg = load_config(f.path()).unwrap();
        match &cfg.transport {
            TransportConfig::Ssh { auth, .. } => {
                assert!(matches!(auth, SshAuth::Password { env_var } if env_var == "SSH_PASSWORD"));
            }
        }
    }
}
