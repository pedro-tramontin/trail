//! Collector config loader. The schema is frozen (see the master plan's
//! "~/.trail/collector.json" block).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CollectorConfig {
    pub inbox_dir: std::path::PathBuf,
    pub processed_dir: std::path::PathBuf,
    pub failed_dir: std::path::PathBuf,
    pub plan_root: std::path::PathBuf,
    pub plan_template: String,
    pub schema_path: std::path::PathBuf,
    pub log_path: std::path::PathBuf,
    pub user: String,
    /// v1 only supports the literal string `"strict"`. v2 may add `"warn"`
    /// / `"off"` variants — don't enumerate them yet (kept as String so
    /// the loader tolerates unknown future values without breaking).
    pub schema_validation: String,
}

#[derive(Debug)]
pub enum ConfigError {
    Io(std::io::Error),
    Parse(serde_json::Error),
    MissingRequiredKey(String),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "IO error: {e}"),
            Self::Parse(e) => write!(f, "invalid JSON: {e}"),
            Self::MissingRequiredKey(keys) => {
                write!(f, "config missing required key(s): {keys}")
            }
        }
    }
}

impl std::error::Error for ConfigError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Parse(e) => Some(e),
            Self::MissingRequiredKey(_) => None,
        }
    }
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        Self::Parse(e)
    }
}

/// The 9 keys the master's frozen `~/.trail/collector.json` schema requires.
/// Used by `load()` to surface missing-key errors with the specific key name(s)
/// before serde's less helpful "missing field" message would surface them.
pub const REQUIRED_KEYS: &[&str] = &[
    "inbox_dir",
    "processed_dir",
    "failed_dir",
    "plan_root",
    "plan_template",
    "schema_path",
    "log_path",
    "user",
    "schema_validation",
];

pub fn load(path: &Path) -> Result<CollectorConfig, ConfigError> {
    let raw = std::fs::read_to_string(path)?;
    let value: serde_json::Value = serde_json::from_str(&raw)?;

    // Surface missing-key errors with the specific key name(s), before
    // serde's less helpful "missing field" message.
    if let serde_json::Value::Object(map) = &value {
        let mut missing = Vec::new();
        for k in REQUIRED_KEYS {
            if !map.contains_key(*k) {
                missing.push((*k).to_string());
            }
        }
        if !missing.is_empty() {
            return Err(ConfigError::MissingRequiredKey(missing.join(", ")));
        }
    }

    let cfg: CollectorConfig = serde_json::from_value(value)?;
    Ok(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_temp(json: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(json.as_bytes()).unwrap();
        f.flush().unwrap();
        f
    }

    fn valid_json() -> &'static str {
        r#"{
            "inbox_dir": "/home/p/inbox",
            "processed_dir": "/home/p/processed",
            "failed_dir": "/home/p/failed",
            "plan_root": "/home/p/.hermes/plans/coaching/daily",
            "plan_template": "{date}.md",
            "schema_path": "/home/p/schema.json",
            "log_path": "/home/p/collector.log",
            "user": "pedro",
            "schema_validation": "strict"
        }"#
    }

    #[test]
    fn load_valid_config() {
        let f = write_temp(valid_json());
        let cfg = load(f.path()).unwrap();
        assert_eq!(cfg.user, "pedro");
        assert_eq!(cfg.plan_template, "{date}.md");
        assert_eq!(cfg.inbox_dir, std::path::PathBuf::from("/home/p/inbox"));
        assert_eq!(cfg.schema_validation, "strict");
    }

    #[test]
    fn missing_file_returns_io_error() {
        let result = load(Path::new("/nonexistent/path/collector.json"));
        assert!(matches!(result, Err(ConfigError::Io(_))));
    }

    #[test]
    fn missing_required_key_returns_missing_required_key() {
        // Drop the `user` key.
        let json = r#"{
            "inbox_dir": "/x", "processed_dir": "/x", "failed_dir": "/x",
            "plan_root": "/x", "plan_template": "{date}.md", "schema_path": "/x",
            "log_path": "/x", "schema_validation": "strict"
        }"#;
        let f = write_temp(json);
        let result = load(f.path());
        match result {
            Err(ConfigError::MissingRequiredKey(s)) => assert!(s.contains("user")),
            other => panic!("expected MissingRequiredKey, got {:?}", other),
        }
    }

    #[test]
    fn invalid_json_returns_parse_error() {
        let f = write_temp("{ not valid json");
        assert!(matches!(load(f.path()), Err(ConfigError::Parse(_))));
    }
}
