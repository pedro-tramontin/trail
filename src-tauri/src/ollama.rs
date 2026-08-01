//! Typed HTTP client for the local `ollama` server.
//!
//! `ollama` is a self-contained inference server that the Phase 3
//! summarizer calls over HTTP. By default it listens on
//! `http://localhost:11434`; this wrapper exposes:
//!
//! * [`OllamaClient::generate`] — POST to `/api/generate` with a system
//!   prompt + a user prompt and return the model's text response.
//! * [`OllamaClient::health_check`] — GET `/api/tags` to confirm the
//!   server is reachable. Surfaces the typed [`OllamaError::NotRunning`]
//!   when the port is closed, so the UI can show a clear "start ollama"
//!   hint instead of a raw `reqwest` error.
//!
//! The error type implements `std::error::Error` manually (rather than
//! pulling in `thiserror` for one enum) to keep this module's
//! dependency surface minimal. The summarizer / scheduler modules
//! already use `thiserror`, so the choice here is stylistic (this
//! module was written first).
//!
//! ## Design contract
//!
//! The request body uses `"stream": false` so `/api/generate` returns a
//! single JSON object (with `"response"` plus bookkeeping fields) rather
//! than an NDJSON stream. That keeps the call signature synchronous
//! from the caller's perspective and lets the response be parsed with
//! `reqwest`'s `json` feature.

use std::fmt;

/// The default `ollama` HTTP endpoint. Exposed as a `const` so tests
/// and the Tauri-side wiring can reference the same value.
pub const DEFAULT_ENDPOINT: &str = "http://localhost:11434";

/// Maximum bytes to read from an error response body. Anything
/// larger is truncated (via `read_capped_body` below) so a
/// misbehaving server can't blow up the log / IPC error with a
/// multi-MB body. `reqwest 0.12` does not ship a
/// `text_with_limit` API, so we read `bytes()` and bound the
/// lossy decode ourselves.
pub const MAX_ERROR_BODY_BYTES: usize = 8 * 1024;

/// Read the response body with a hard byte cap. Returns a UTF-8
/// lossy-decoded string no longer than `cap` bytes; if the body
/// was larger, the string is suffixed with a marker so the
/// operator knows the response was truncated. Decode errors
/// surface as the placeholder so the caller still gets an error
/// message rather than panicking.
async fn read_capped_body(resp: reqwest::Response, cap: usize) -> String {
    let bytes = match resp.bytes().await {
        Ok(b) => b,
        Err(_) => return "<unreadable>".to_string(),
    };
    if bytes.len() <= cap {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    // Truncated: decode up to `cap` and append a marker.
    let truncated = String::from_utf8_lossy(&bytes[..cap]).into_owned();
    format!("{truncated}... <truncated, body was {} bytes>", bytes.len())
}

/// Typed errors from the ollama client. The three variants cover the
/// three failure modes the caller actually needs to distinguish:
///
/// * `NotRunning` — the TCP connect failed (port closed, no listener).
///   The UI should surface a "is ollama running?" hint.
/// * `Http(_)` — ollama returned a non-2xx status (e.g. 404 model not
///   found, 500 internal). The wrapped string includes the status code
///   and the response body so logs are actionable.
/// * `EmptyResponse` — ollama returned 200 but the `"response"` field
///   was empty. The model produced no usable text.
#[derive(Debug)]
pub enum OllamaError {
    NotRunning,
    Http(String),
    EmptyResponse,
}

impl fmt::Display for OllamaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OllamaError::NotRunning => write!(
                f,
                "ollama is not running at the configured endpoint (could not connect)"
            ),
            OllamaError::Http(detail) => write!(f, "ollama HTTP error: {detail}"),
            OllamaError::EmptyResponse => {
                write!(
                    f,
                    "ollama returned a 200 response with no usable model output \
                     (empty body, non-JSON body, or empty `response` field)"
                )
            }
        }
    }
}

impl std::error::Error for OllamaError {}

/// JSON body we POST to `/api/generate`. `stream: false` is the
/// important bit — it tells ollama to return a single JSON object
/// instead of an NDJSON stream.
#[derive(Debug, serde::Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    system: &'a str,
    prompt: &'a str,
    stream: bool,
}

/// JSON body ollama returns from `/api/generate` when `stream: false`.
/// We only care about the `response` field; the other fields (`done`,
/// `context`, timing, etc.) are bookkeeping we ignore.
#[derive(Debug, serde::Deserialize)]
struct GenerateResponse {
    response: String,
}

/// Thin HTTP client for the local ollama server.
///
/// `endpoint` is the base URL (no trailing slash) — e.g.
/// `http://localhost:11434`. `http` is the underlying `reqwest::Client`
/// (kept as a public field so tests can reuse the same client against
/// the `wiremock` mock server's base URL).
#[derive(Debug, Clone)]
pub struct OllamaClient {
    pub endpoint: String,
    pub http: reqwest::Client,
}

impl OllamaClient {
    /// Build a client targeting `endpoint` (e.g. `http://localhost:11434`).
    /// The underlying `reqwest::Client` uses the default timeout / TLS
    /// settings — the only thing we customize is that it's available
    /// pre-built so all requests share a connection pool.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self {
            endpoint: endpoint.into(),
            http: reqwest::Client::new(),
        }
    }

    /// Send a generation request to `{endpoint}/api/generate` and return
    /// the model's text response. See module docs for the request shape.
    pub async fn generate(
        &self,
        system: &str,
        prompt: &str,
        model: &str,
    ) -> Result<String, OllamaError> {
        let url = format!("{}/api/generate", self.endpoint);
        let body = GenerateRequest {
            model,
            system,
            prompt,
            stream: false,
        };
        // Map the network-level error to `NotRunning` so the caller can
        // branch on the typed variant instead of matching on reqwest
        // error text. `send()` itself only fails on the request build /
        // connect stage; 4xx/5xx come back through the Ok arm.
        let resp = match self.http.post(&url).json(&body).send().await {
            Ok(r) => r,
            Err(_) => return Err(OllamaError::NotRunning),
        };
        let status = resp.status();
        if !status.is_success() {
            // Read the body (best-effort) so the error message
            // includes what ollama told us. Cap the read at
            // MAX_ERROR_BODY_BYTES so a misbehaving server can't
            // blow up the log / IPC error with a multi-MB error
            // body (see `read_capped_body`).
            let body = read_capped_body(resp, MAX_ERROR_BODY_BYTES).await;
            return Err(OllamaError::Http(format!("status {status}: {body}")));
        }
        let parsed: GenerateResponse = match resp.json().await {
            Ok(v) => v,
            Err(_) => {
                // Non-JSON success — treat as empty since the model
                // produced nothing usable.
                return Err(OllamaError::EmptyResponse);
            }
        };
        if parsed.response.is_empty() {
            return Err(OllamaError::EmptyResponse);
        }
        Ok(parsed.response)
    }

    /// Confirm the ollama server is reachable. Returns `Ok(())` on 2xx,
    /// `NotRunning` on a connect failure, `Http(_)` on a non-2xx.
    pub async fn health_check(&self) -> Result<(), OllamaError> {
        let url = format!("{}/api/tags", self.endpoint);
        let resp = match self.http.get(&url).send().await {
            Ok(r) => r,
            Err(_) => return Err(OllamaError::NotRunning),
        };
        let status = resp.status();
        if !status.is_success() {
            return Err(OllamaError::Http(format!("status {status}")));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Test 1: a 200 with a valid JSON body returns the `response` field.
    #[tokio::test]
    async fn generate_returns_response_body_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": "ok text",
                "done": true,
            })))
            .mount(&server)
            .await;

        let client = OllamaClient::new(server.uri());
        let out = client
            .generate("you are a summarizer", "summarize today", "llama3")
            .await
            .expect("200 + valid JSON should yield Ok");
        assert_eq!(out, "ok text");
    }

    /// Test 2: a closed port surfaces as `NotRunning`, not a raw
    /// `reqwest::Error`. We bind a TCP listener on an ephemeral
    /// port and immediately drop it, leaving the port free but
    /// with no listener — the connect attempt then refuses. This
    /// is more robust than hard-coding port 1, which can be flaky
    /// in some environments (firewall rules, etc.).
    #[tokio::test]
    async fn health_check_returns_not_running_when_unreachable() {
        // Pick a free port, then drop the listener so the port is
        // unbound for the next `connect()`. There is a small race
        // where another process could grab the port, but it's
        // microseconds wide and the test's only assertion is
        // "connect fails".
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let endpoint = format!("http://127.0.0.1:{port}");
        let client = OllamaClient::new(endpoint);
        let err = client
            .health_check()
            .await
            .expect_err("connection to unbound port should fail");
        assert!(
            matches!(err, OllamaError::NotRunning),
            "expected NotRunning, got {err:?}"
        );
    }

    /// Test 3: a 4xx from ollama surfaces as `Http(_)` and the message
    /// includes the status code so the operator can diagnose.
    #[tokio::test]
    async fn generate_returns_http_error_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .respond_with(ResponseTemplate::new(404).set_body_string("model not found"))
            .mount(&server)
            .await;

        let client = OllamaClient::new(server.uri());
        let err = client
            .generate("system", "prompt", "missing-model")
            .await
            .expect_err("404 should yield Err");
        match err {
            OllamaError::Http(msg) => {
                assert!(
                    msg.contains("404"),
                    "HTTP error message should include status code, got: {msg}"
                );
            }
            other => panic!("expected Http(_), got {other:?}"),
        }
    }

    /// Test 4: a 200 with `"response": ""` (or a non-string/empty
    /// body) surfaces as `EmptyResponse` so the caller can branch on
    /// "model produced nothing" without parsing the raw text.
    #[tokio::test]
    async fn generate_returns_empty_response_on_blank_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "response": "",
                "done": true,
            })))
            .mount(&server)
            .await;

        let client = OllamaClient::new(server.uri());
        let err = client
            .generate("system", "prompt", "llama3")
            .await
            .expect_err("empty response field should yield Err");
        assert!(
            matches!(err, OllamaError::EmptyResponse),
            "expected EmptyResponse, got {err:?}"
        );
    }

    /// Test 5: a 4xx with a body larger than MAX_ERROR_BODY_BYTES
    /// must not blow up the IPC error string. Pre-fix the body
    /// was read with `text()` (unbounded); the fix wraps the body
    /// read in `read_capped_body(..., MAX_ERROR_BODY_BYTES)`
    /// which truncates beyond the cap (and appends a marker so
    /// operators can see it happened).
    #[tokio::test]
    async fn generate_caps_error_body_at_max_bytes() {
        let server = MockServer::start().await;
        let huge_body = "x".repeat(MAX_ERROR_BODY_BYTES * 4);
        Mock::given(method("POST"))
            .and(path("/api/generate"))
            .respond_with(ResponseTemplate::new(500).set_body_string(huge_body))
            .mount(&server)
            .await;

        let client = OllamaClient::new(server.uri());
        let err = client
            .generate("system", "prompt", "llama3")
            .await
            .expect_err("500 should yield Err");
        match err {
            OllamaError::Http(msg) => {
                // The formatted error must include the status and
                // the truncated-body marker so the operator can
                // tell what happened.
                assert!(msg.contains("500"));
                assert!(
                    msg.contains("<truncated,"),
                    "expected truncation marker in error, got: {msg}"
                );
                // The body is truncated at MAX_ERROR_BODY_BYTES
                // plus a fixed marker string ("... <truncated,
                // body was N bytes>"), so the total msg length
                // is bounded by ~MAX_ERROR_BODY_BYTES + a small
                // constant for the status prefix + marker.
                assert!(
                    msg.len() < MAX_ERROR_BODY_BYTES + 200,
                    "HTTP error msg is {} bytes (cap = {} + 200)",
                    msg.len(),
                    MAX_ERROR_BODY_BYTES
                );
            }
            other => panic!("expected Http(_), got {other:?}"),
        }
    }
}
