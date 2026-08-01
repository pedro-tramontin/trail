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
    pub calendar_ics: PathBuf,
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
    /// watch. Phase C maps the first one to [`Config::calendar_ics`]
    /// and carries the rest here.
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
    let config: Config = serde_json::from_str(&contents)?;
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
