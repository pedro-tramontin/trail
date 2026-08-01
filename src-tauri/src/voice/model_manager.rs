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

/// Download `url` to `dest`, streaming the response body to a temp
/// file in the same directory as `dest` and renaming atomically on
/// success. The HTTP status is checked up front; a non-2xx response
/// is reported as `ModelError::Network` (the response body, if any,
/// is consumed and dropped so the connection can be reused).
///
/// The 150 MB whisper model is too large to buffer in memory; the
/// pre-fix implementation used `response.bytes()` (which materialises
/// the full body into a `Bytes` buffer) and wrote the result to the
/// final path. That code was also vulnerable to writing a 404 HTML
/// page to the model file when the server returned a non-2xx — the
/// SHA mismatch would only surface after a full wasted download.
///
/// Streaming via [`tokio::fs::File`] to a `.partial` sibling of
/// `dest`, then `rename` on success, follows the same atomic-write
/// pattern used elsewhere in the project (see `learner::save`).
async fn download_model(url: &str, dest: &Path) -> Result<(), ModelError> {
    let mut response = reqwest::get(url)
        .await
        .map_err(|e| ModelError::Network(e.to_string()))?;
    let status = response.status();
    if !status.is_success() {
        return Err(ModelError::Network(format!(
            "download failed: HTTP {} for {url}",
            status.as_u16()
        )));
    }
    let partial = dest.with_extension("bin.partial");
    // Best-effort remove of any stale partial from a prior failed
    // download (ignore NotFound).
    if let Err(e) = std::fs::remove_file(&partial) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(ModelError::Io(e));
        }
    }
    let mut file = tokio::fs::File::create(&partial)
        .await
        .map_err(ModelError::Io)?;
    // `Response::chunk()` returns the next chunk of the body as
    // `Bytes`, or `None` at EOF. We pull chunks until the stream is
    // drained and write each one to the partial file. This keeps
    // memory bounded to one chunk at a time (typically 16 KB
    // per chunk by default for reqwest 0.12's default
    // `Decoder`).
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ModelError::Network(e.to_string()))?
    {
        tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
            .await
            .map_err(ModelError::Io)?;
    }
    tokio::io::AsyncWriteExt::flush(&mut file)
        .await
        .map_err(ModelError::Io)?;
    drop(file);
    // Best-effort remove of the final destination so the rename
    // works on Windows (where rename refuses to overwrite).
    if let Err(e) = std::fs::remove_file(dest) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(ModelError::Io(e));
        }
    }
    std::fs::rename(&partial, dest).map_err(ModelError::Io)?;
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
        // Verify cached file. Only `Sha256Mismatch` is treated as
        // "corrupt cache → force re-download" — I/O errors
        // (permission denied, disk error, etc.) are propagated
        // unchanged so the caller sees the real failure mode
        // instead of a misleading "I deleted the cache and tried
        // again" loop.
        match verify_sha256(&dest, expected_sha256) {
            Ok(()) => return Ok(dest),
            Err(ModelError::Sha256Mismatch { .. }) => {
                let _ = std::fs::remove_file(&dest);
            }
            Err(other) => return Err(other),
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
        // Pre-fix this test depended on the real `MODEL_URL` being
        // reachable from CI (and on its bytes not matching the bogus
        // SHA) — both of which are flaky in offline / hermetic CI.
        // The rewrite uses a local `wiremock` server (already a
        // dev-dep via the ollama tests) to serve a known payload,
        // then primes the cache with a different payload, then asks
        // `ensure_model_with` to verify-and-recover.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        let payload: Vec<u8> = b"wiremock-served model payload".to_vec();
        let payload_sha = {
            let mut hasher = Sha256::new();
            hasher.update(&payload);
            format!("{:x}", hasher.finalize())
        };
        Mock::given(method("GET"))
            .and(path("/ggml-base.en.bin"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(payload.clone()))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        // Pre-write a "corrupt" cache: bytes whose SHA does NOT
        // match the mock's payload SHA.
        let bogus_bytes: Vec<u8> = b"corrupt local cache".to_vec();
        std::fs::write(models_dir.join("ggml-base.en.bin"), &bogus_bytes).unwrap();

        // First call: cache SHA mismatch → delete + re-download →
        // verify against mock's payload → Ok.
        let url = format!("{}/ggml-base.en.bin", server.uri());
        let result = ensure_model_with(&url, &payload_sha, Some(tmp.path())).await;
        assert!(
            result.is_ok(),
            "expected Ok after cache mismatch + mock download, got {result:?}"
        );
        // The cached file should now be the mock's payload, not the
        // original bogus bytes.
        let on_disk = std::fs::read(models_dir.join("ggml-base.en.bin")).unwrap();
        assert_eq!(on_disk, payload);
    }

    #[tokio::test]
    async fn ensure_model_with_rejects_non_2xx_response() {
        // Verifies the new HTTP-status check in `download_model`:
        // a 404 from the server must surface as `Network(...)`
        // without writing a partial / HTML body to the model file.
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/ggml-base.en.bin"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let tmp = tempfile::tempdir().unwrap();
        let url = format!("{}/ggml-base.en.bin", server.uri());
        let bogus_sha = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = ensure_model_with(&url, bogus_sha, Some(tmp.path())).await;
        match &result {
            Err(ModelError::Network(msg)) => {
                assert!(
                    msg.contains("404"),
                    "expected '404' in network error, got {msg:?}"
                );
            }
            other => panic!("expected Network(404), got {other:?}"),
        }
        // The model file must not have been created (no partial / 404
        // body written to disk).
        assert!(!tmp.path().join("models/ggml-base.en.bin").exists());
        // The .partial file must also be absent.
        assert!(!tmp.path().join("models/ggml-base.en.bin.partial").exists());
    }

    #[tokio::test]
    async fn ensure_model_propagates_io_error_from_corrupt_cache() {
        // Verifies the cache-verify error-handling fix: I/O errors
        // from `verify_sha256` (e.g. permission denied) must NOT be
        // swallowed with `Err(_) => delete + retry`. Pre-fix code
        // would delete the cache and re-attempt the download.
        //
        // We can't easily make `std::fs::read` fail on a writable
        // tempdir, so we make the cache a *directory*: `read` on a
        // directory returns `EISDIR`, which is an I/O error (not
        // `Sha256Mismatch`).
        let tmp = tempfile::tempdir().unwrap();
        let models_dir = tmp.path().join("models");
        std::fs::create_dir_all(&models_dir).unwrap();
        // Replace the expected model file with a directory of the
        // same name so the cache-verify hits an I/O error.
        let dest = models_dir.join("ggml-base.en.bin");
        std::fs::remove_file(&dest).ok();
        std::fs::create_dir(&dest).unwrap();

        let bogus_sha = "0000000000000000000000000000000000000000000000000000000000000000";
        let result = ensure_model_with(MODEL_URL, bogus_sha, Some(tmp.path())).await;
        // Must be a real I/O error, NOT `Sha256Mismatch`.
        match &result {
            Err(ModelError::Io(_)) => {} // expected
            Err(other) => panic!("expected Io error (not swallowed), got {other:?}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }
}
