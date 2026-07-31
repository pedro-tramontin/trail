//! `--health` mode: verify config + paths + schema are sane.

use crate::config::CollectorConfig;
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub ok: bool,
    pub user: String,
    pub inbox: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// Run the health check. Returns the report; the caller decides the exit code.
pub fn check(cfg: &CollectorConfig) -> HealthReport {
    let mut errors = Vec::new();

    for key in ["inbox_dir", "processed_dir", "failed_dir", "plan_root"] {
        let p = match key {
            "inbox_dir" => &cfg.inbox_dir,
            "processed_dir" => &cfg.processed_dir,
            "failed_dir" => &cfg.failed_dir,
            "plan_root" => &cfg.plan_root,
            _ => unreachable!(),
        };
        if !p.exists() {
            errors.push(format!("{key} does not exist: {}", p.display()));
        } else if !p.is_dir() {
            errors.push(format!("{key} is not a directory: {}", p.display()));
        }
    }

    if !cfg.schema_path.exists() {
        errors.push(format!(
            "schema_path does not exist: {}",
            cfg.schema_path.display()
        ));
    }

    // The log_path's parent must be writable. The log itself may not exist yet.
    if let Some(parent) = cfg.log_path.parent() {
        if !parent.exists() {
            errors.push(format!(
                "log_path parent does not exist: {}",
                parent.display()
            ));
        } else {
            // Probe writability with a metadata check (cross-platform).
            match std::fs::metadata(parent) {
                Ok(md) if !md.permissions().readonly() => {} // writable
                Ok(_) => errors.push(format!(
                    "log_path parent not writable: {}",
                    parent.display()
                )),
                Err(e) => errors.push(format!("log_path parent stat failed: {e}")),
            }
        }
    }

    if !cfg.plan_template.contains("{date}") {
        errors.push("plan_template must contain '{date}' placeholder".to_string());
    }

    HealthReport {
        ok: errors.is_empty(),
        user: cfg.user.clone(),
        inbox: cfg.inbox_dir.display().to_string(),
        errors,
    }
}

/// Run the health check and print the JSON report to stdout.
/// Returns the process exit code (0 = healthy, 1 = errors).
pub fn run(cfg: &CollectorConfig) -> i32 {
    let report = check(cfg);
    // Use println! + serde_json manually to keep --health dependency-free
    // (no need to pull in `colored` or `indicatif` for one print).
    match serde_json::to_string_pretty(&report) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("health: failed to serialize report: {e}");
            return 1;
        }
    }
    if report.ok {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_valid_cfg(tmp: &std::path::Path) -> CollectorConfig {
        CollectorConfig {
            inbox_dir: tmp.join("inbox"),
            processed_dir: tmp.join("processed"),
            failed_dir: tmp.join("failed"),
            plan_root: tmp.join("plans"),
            plan_template: "{date}.md".to_string(),
            schema_path: tmp.join("schema.json"),
            log_path: tmp.join("collector.log"),
            user: "pedro".to_string(),
            schema_validation: "strict".to_string(),
        }
    }

    #[test]
    fn health_ok_when_all_paths_exist() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = make_valid_cfg(tmp.path());
        // Create the dirs and a stub schema.
        for key in ["inbox", "processed", "failed", "plans"] {
            fs::create_dir_all(tmp.path().join(key)).unwrap();
        }
        fs::write(&cfg.schema_path, "{}").unwrap();
        let report = check(&cfg);
        assert!(report.ok, "expected ok, got errors: {:?}", report.errors);
        assert_eq!(report.user, "pedro");
    }

    #[test]
    fn health_fails_when_inbox_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = make_valid_cfg(tmp.path());
        // Note: do NOT create inbox_dir.
        for key in ["processed", "failed", "plans"] {
            fs::create_dir_all(tmp.path().join(key)).unwrap();
        }
        fs::write(&cfg.schema_path, "{}").unwrap();
        let report = check(&cfg);
        assert!(!report.ok);
        assert!(report.errors.iter().any(|e| e.contains("inbox_dir")));
    }

    #[test]
    fn health_fails_when_plan_template_has_no_date_placeholder() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = make_valid_cfg(tmp.path());
        for key in ["inbox", "processed", "failed", "plans"] {
            fs::create_dir_all(tmp.path().join(key)).unwrap();
        }
        fs::write(&cfg.schema_path, "{}").unwrap();
        cfg.plan_template = "today.md".to_string();
        let report = check(&cfg);
        assert!(!report.ok);
        assert!(report.errors.iter().any(|e| e.contains("plan_template")));
    }

    #[test]
    fn health_fails_when_schema_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = make_valid_cfg(tmp.path());
        for key in ["inbox", "processed", "failed", "plans"] {
            fs::create_dir_all(tmp.path().join(key)).unwrap();
        }
        // Note: do NOT create schema_path.
        let report = check(&cfg);
        assert!(!report.ok);
        assert!(report.errors.iter().any(|e| e.contains("schema_path")));
    }
}
