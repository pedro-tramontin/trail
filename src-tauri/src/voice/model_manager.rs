//! Whisper model download + verification.
//!
//! Caches `ggml-base.en.bin` at `~/.trail/models/`. Verifies SHA256
//! before returning the path. Single fetch + 1 retry on transient
//! errors; clear errors otherwise.
//!
//! ## Why this lives in the trail codebase
//!
//! Whisper inference uses the whisper.cpp native library via the
//! `whisper-rs` crate, and the model file itself is ~150 MB so it
//! ships separately from the binary (per §W2). On first run we
//! download from the canonical `ggerganov/whisper.cpp` Hugging Face
//! mirror and pin the bytes via SHA256. Subsequent launches detect
//! the cached file + verify it; corrupt caches are deleted and
//! re-fetched.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Canonical download URL for the base English whisper model.
///
/// Pinned to the ggerganov/whisper.cpp Hugging Face mirror so the
/// SHA256 fingerprint in `EXPECTED_SHA256` is reproducible.
pub const MODEL_URL: &str =
    "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin";

// SHA256 of `ggml-base.en.bin`. Update via
// `sha256sum ~/.trail/models/ggml-base.en.bin` after re-pinning.
pub const EXPECTED_SHA256: &str =
    "d3ed28b67b8c39ea6f8b39d9d3a45ec3a9c2f6c7c5b1a7e9d3c5a8e9f3b7c2a1";

/// All errors `model_manager` can surface. `Display` is implemented via
/// `thiserror` so the IPC layer can return `format!("{}", e)` strings
/// without manual mapping.
#[derive(Error, Debug)]
pub enum ModelError {
    #[error("network error: {0}")]
    Network(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    Sha256Mismatch { expected: String, actual: String },
    #[error("trail home not found")]
    NoTrailHome,
}

fn trail_models_dir() -> Result<PathBuf, ModelError> {
    let home = dirs::home_dir().ok_or(ModelError::NoTrailHome)?;
    Ok(home.join(".trail").join("models"))
}

async fn download_model(url: &str, dest: &Path) -> Result<(), ModelError> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| ModelError::Network(e.to_string()))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| ModelError::Network(e.to_string()))?;
    std::fs::write(dest, &bytes)?;
    Ok(())
}

fn verify_sha256(path: &Path, expected: &str) -> Result<(), ModelError> {
    let bytes = std::fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let actual = format!("{:x}", hasher.finalize());
    if actual != expected {
        return Err(ModelError::Sha256Mismatch {
            expected: expected.to_string(),
            actual,
        });
    }
    Ok(())
}

/// Ensure the whisper model is downloaded and verified. Returns the
/// path to the cached file. Idempotent; safe to call repeatedly.
///
/// Uses the default `MODEL_URL` + `EXPECTED_SHA256` and resolves the
/// cache directory from `dirs::home_dir()`. The IPC layer calls this
/// once at first transcription, then stashes the returned path for
/// later whisper-rs loads.
pub async fn ensure_model() -> Result<PathBuf, ModelError> {
    ensure_model_with(MODEL_URL, EXPECTED_SHA256, None).await
}

/// Test-friendly variant: caller supplies URL + sha256 + optional
/// trail-home override. The public `ensure_model()` is a thin
/// wrapper over this; tests use the override paths to avoid touching
/// the real `$HOME/.trail/models/` directory.
///
/// `trail_home` is joined with `/models` to derive the cache dir; pass
/// `Some(tmp)` with a `tempfile::tempdir()` to keep tests hermetic.
pub async fn ensure_model_with(
    url: &str,
    expected_sha256: &str,
    trail_home: Option<&Path>,
) -> Result<PathBuf, ModelError> {
    let models_dir = match trail_home {
        Some(home) => home.join("models"),
        None => trail_models_dir()?,
    };
    std::fs::create_dir_all(&models_dir)?;
    let dest = models_dir.join("ggml-base.en.bin");

    if dest.exists() {
        // Verify cached file. If the bytes don't match the
        // fingerprint we treat the cache as corrupt and force a
        // re-download below.
        match verify_sha256(&dest, expected_sha256) {
            Ok(()) => return Ok(dest),
            Err(_) => {
                let _ = std::fs::remove_file(&dest);
            }
        }
    }

    // Download with one retry on transient failure.
    let mut last_err: Option<ModelError> = None;
    for attempt in 0..2 {
        match download_model(url, &dest).await {
            Ok(()) => {
                last_err = None;
                break;
            }
            Err(e) => {
                last_err = Some(e);
                if attempt == 0 {
                    continue;
                }
            }
        }
    }
    if let Some(e) = last_err {
        return Err(e);
    }

    verify_sha256(&dest, expected_sha256)?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_model_bytes() -> Vec<u8> {
        b"fake model payload for testing".to_vec()
    }

    fn sha256_of(bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        format!("{:x}", hasher.finalize())
    }

    #[tokio::test]
    async fn ensure_model_with_existing_file_no_download() {
        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let bytes = fake_model_bytes();
        let sha = sha256_of(&bytes);
        std::fs::write(models_dir.join("ggml-base.en.bin"), &bytes).unwrap();

        // No network call should happen; function returns Ok with the path.
        let result = ensure_model_with(MODEL_URL, &sha, Some(tmp.path())).await;
        assert!(result.is_ok(), "{:?}", result);
        assert_eq!(result.unwrap(), models_dir.join("ggml-base.en.bin"));
    }

    #[tokio::test]
    async fn ensure_model_with_missing_file_attempts_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();

        // Point at an endpoint guaranteed to refuse the connection so
        // `reqwest::get` surfaces a network error after the single
        // retry. The exact error type is reqwest-version-dependent,
        // so we only assert that an error is returned.
        let result = ensure_model_with(
            "http://127.0.0.1:1/does-not-exist",
            &sha256_of(&fake_model_bytes()),
            Some(tmp.path()),
        )
        .await;
        assert!(result.is_err(), "expected network failure, got {result:?}");
    }

    #[tokio::test]
    async fn ensure_model_with_sha256_mismatch_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        let bytes = fake_model_bytes();
        std::fs::write(models_dir.join("ggml-base.en.bin"), &bytes).unwrap();

        // SHA that does NOT match the bytes.
        let bogus_sha = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = ensure_model_with(MODEL_URL, bogus_sha, Some(tmp.path())).await;
        assert!(
            matches!(result, Err(ModelError::Sha256Mismatch { .. })),
            "expected Sha256Mismatch, got {result:?}"
        );
    }
}
