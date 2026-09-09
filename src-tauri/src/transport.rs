//! SSH transport for pushing day summaries to the VPS.
//!
//! The `Transport` trait surface is frozen in the master plan: `name` +
//! `push` + `health_check`. v1 only ships `SshTransport`; v2 will add
//! `HttpsTransport` / `S3Transport` / `DatabaseTransport` cases to
//! `from_config`. The `TransportError` enum is `#[non_exhaustive]` so
//! those v2 transports can add their own variants without breaking
//! v1 callers.

use async_trait::async_trait;
use std::path::{Path, PathBuf};
use thiserror::Error;
// `Zeroizing` is only referenced by `load_private_key_pem`, which is
// gated `#[cfg(unix)]` (see its doc comment). On Windows the gate
// excludes the function, so the import would otherwise trigger an
// `unused_imports` warning under the clippy `-D warnings` gate and
// an `unused_imports` warning under `RUSTFLAGS=-D warnings` on the
// draft-build Windows job. Gate the import to match.
#[cfg(unix)]
use zeroize::Zeroizing;

use crate::config::{SshAuth, TransportConfig};

/// Errors that any transport can surface. `#[non_exhaustive]` lets
/// v2 transports add their own variants (e.g. `S3(String)`, `Https(String)`)
/// without breaking v1 callers that match on the v1 variants with a
/// non-default arm.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum TransportError {
    #[error("ssh operation failed: {0}")]
    Ssh(String),
    #[error("transport config error: {0}")]
    Config(String),
    #[error("I/O error: {0}")]
    Io(String),
    /// The server's host key is not yet pinned in `known_hosts`. Expected
    /// on first connect — the caller should surface the fingerprint and
    /// offer a trust-on-first-use prompt rather than fail hard.
    #[error("unknown host key for {host}:{port} (fingerprint {fingerprint}) — not yet pinned")]
    HostKeyUnknown {
        host: String,
        port: u16,
        fingerprint: String,
    },
    /// The server's host key changed since onboarding. Never expected —
    /// must be a hard, non-dismissible stop (possible man-in-the-middle).
    #[error("HOST KEY MISMATCH for {host}:{port} — refusing to connect")]
    HostKeyMismatch { host: String, port: u16 },
}

/// The transport contract for pushing payloads to the VPS.
///
/// The trait is `Send + Sync` so callers can stash a `Box<dyn Transport>`
/// behind a Tauri state handle. Methods are `async` because every
/// realistic transport touches the network / filesystem off the
/// hot path; even the local-only path uses `tokio::task::spawn_blocking`.
#[async_trait]
pub trait Transport: Send + Sync + std::fmt::Debug {
    /// Stable identifier for diagnostics + the frontend. `"ssh"` for v1.
    fn name(&self) -> &'static str;

    /// Write `payload` to `<remote_path>/<remote_name>` on the remote side.
    /// Returns `Ok(())` once the remote write succeeds.
    async fn push(&self, payload: &[u8], remote_name: &str) -> Result<(), TransportError>;

    /// Liveness probe: prove the transport can reach the remote + auth
    /// works, without writing any file. Returns `Ok(())` on success.
    async fn health_check(&self) -> Result<(), TransportError>;
}

/// Factory: build the right transport from a frozen config schema.
///
/// v1 only matches `Ssh`; v2 will add `Https`/`S3`/`Database` cases.
pub fn from_config(cfg: &TransportConfig) -> Result<Box<dyn Transport>, TransportError> {
    match cfg {
        TransportConfig::Ssh {
            host,
            port,
            user,
            auth,
            remote_path,
            known_hosts,
        } => Ok(Box::new(SshTransport::new(
            host.clone(),
            *port,
            user.clone(),
            auth.clone(),
            remote_path.clone(),
            known_hosts.clone(),
        ))),
    }
}

/// SSH-based transport. Clones the config fields and reads the
/// private key from the macOS Keychain on each `push()` call — the
/// private key never lands on disk (per the master's security
/// tradeoffs).
#[derive(Clone, Debug)]
pub struct SshTransport {
    host: String,
    port: u16,
    user: String,
    auth: SshAuth,
    remote_path: PathBuf,
    known_hosts: PathBuf,
}

impl SshTransport {
    pub fn new(
        host: String,
        port: u16,
        user: String,
        auth: SshAuth,
        remote_path: PathBuf,
        known_hosts: PathBuf,
    ) -> Self {
        Self {
            host,
            port,
            user,
            auth,
            remote_path,
            known_hosts,
        }
    }

    /// Pull the OpenSSH-encoded private key (PEM string) from the
    /// keychain. The key never leaves the keychain on disk — only
    /// this transient string is held for the duration of one SSH
    /// operation, and `ssh2::Session::userauth_pubkey_memory` accepts
    /// PEM-in-memory directly (no on-disk write, no third-party parser).
    ///
    /// The returned `Zeroizing<String>` is wiped on Drop — defending
    /// against a heap-dump leak of the private key even if the
    /// `keyring 3.x` upstream API returns a plain `String` (which it
    /// does today).
    ///
    /// `#[cfg(unix)]` because both call sites (the `push` and the
    /// `health_check` async fns further down) are gated on unix —
    /// pubkey-in-memory auth is not implemented for Windows yet
    /// (see `cfg(not(unix))` branches in `push` returning an
    /// explicit "Windows pubkey auth is not supported in v1" error).
    /// Gating the method to match gates the dead-code warning that
    /// fires on `cargo check --target x86_64-pc-windows-msvc` and
    /// on the GitHub Actions Windows runner for the promote.yml
    /// Windows job.
    #[cfg(unix)]
    fn load_private_key_pem(&self) -> Result<Zeroizing<String>, TransportError> {
        let entry = keyring::Entry::new(
            crate::keyring::KEYCHAIN_SERVICE,
            crate::keyring::KEYCHAIN_ACCOUNT,
        )
        .map_err(|e| TransportError::Ssh(format!("keychain open: {e}")))?;
        match entry.get_password() {
            Ok(pem) => Ok(Zeroizing::new(pem)),
            Err(keyring::Error::NoEntry) => Err(TransportError::Ssh(
                "SSH key not generated yet — run onboarding first".into(),
            )),
            Err(e) => Err(TransportError::Ssh(format!("keychain read: {e}"))),
        }
    }
}

#[async_trait]
impl Transport for SshTransport {
    fn name(&self) -> &'static str {
        "ssh"
    }

    async fn push(&self, payload: &[u8], remote_name: &str) -> Result<(), TransportError> {
        // Clone everything into the closure because `ssh2` is blocking
        // and we cannot hold a borrow across `spawn_blocking`.
        let host = self.host.clone();
        let port = self.port;
        // `pem` is only used inside the unix-gated
        // `userauth_pubkey_memory` call below (Windows builds use the
        // password-only path or bail out early with the "pubkey-in-memory
        // auth requires unix" error). Gate it with `#[cfg(unix)]` to
        // silence the Windows-only "unused variable" warning. `user` is
        // still used by both pubkey auth (unix) AND password auth
        // (cross-platform) so we leave it unconditional.
        let user = self.user.clone();
        #[cfg(unix)]
        let pem = self.load_private_key_pem()?;
        let auth = self.auth.clone();
        let remote_path = self.remote_path.clone();
        let known_hosts = self.known_hosts.clone();
        let remote_name = remote_name.to_string();
        let payload = payload.to_vec();

        tokio::task::spawn_blocking(move || -> Result<(), TransportError> {
            use ssh2::Session;
            use std::io::Write;

            let tcp = std::net::TcpStream::connect((host.as_str(), port))
                .map_err(|e| TransportError::Ssh(format!("tcp connect {host}:{port}: {e}")))?;
            let mut sess = Session::new()
                .map_err(|e| TransportError::Ssh(format!("session new: {e}")))?;
            sess.set_tcp_stream(tcp);
            sess.handshake()
                .map_err(|e| TransportError::Ssh(format!("handshake: {e}")))?;

            // Verify the server's host key against the pinned known_hosts
            // entry before we send any credentials.
            check_host_key(&sess, &host, port, &known_hosts)?;

            // Auth.
            match &auth {
                SshAuth::PublicKey { .. } => {
                    // `userauth_pubkey_memory` parses the PEM-in-string
                    // itself (libssh2's
                    // `libssh2_userauth_publickey_frommemory`); with
                    // `pubkey = None` it derives the public key from the
                    // private key bytes. Available on Unix and on Windows
                    // when the `vendored-openssl`/`openssl-on-win32`
                    // features are enabled.
                    #[cfg(unix)]
                    {
                        sess.userauth_pubkey_memory(&user, None, &pem, None)
                            .map_err(|e| TransportError::Ssh(format!("pubkey auth: {e}")))?;
                    }
                    #[cfg(not(unix))]
                    {
                        return Err(TransportError::Ssh(
                            "pubkey-in-memory auth requires unix in v1; Windows builds are not supported yet".into(),
                        ));
                    }
                }
                SshAuth::Password { env_var } => {
                    let password = std::env::var(env_var)
                        .map_err(|_| TransportError::Config(format!("env var {env_var} not set")))?;
                    sess.userauth_password(&user, &password)
                        .map_err(|e| TransportError::Ssh(format!("password auth: {e}")))?;
                }
            }

            // SFTP write. `remote_path` always ends with `/` per the design doc.
            let sftp = sess
                .sftp()
                .map_err(|e| TransportError::Io(format!("sftp open: {e}")))?;
            let remote = remote_path.join(&remote_name);
            let mut file = sftp
                .create(&remote)
                .map_err(|e| TransportError::Io(format!("create {}: {e}", remote.display())))?;
            file.write_all(&payload)
                .map_err(|e| TransportError::Io(format!("write {}: {e}", remote.display())))?;
            Ok(())
        })
        .await
        .map_err(|e| TransportError::Ssh(format!("join: {e}")))?
    }

    async fn health_check(&self) -> Result<(), TransportError> {
        let host = self.host.clone();
        let port = self.port;
        // `user` and `pem` are only used inside the unix-gated
        // `userauth_pubkey_memory` call below (Windows builds bail out
        // early with the "pubkey-in-memory auth requires unix" error).
        // Gate them with `#[cfg(unix)]` to silence the Windows-only
        // "unused variable" warnings.
        #[cfg(unix)]
        let user = self.user.clone();
        #[cfg(unix)]
        let pem = self.load_private_key_pem()?;
        let known_hosts = self.known_hosts.clone();

        tokio::task::spawn_blocking(move || -> Result<(), TransportError> {
            use ssh2::Session;

            let tcp = std::net::TcpStream::connect((host.as_str(), port))
                .map_err(|e| TransportError::Ssh(format!("tcp connect {host}:{port}: {e}")))?;
            let mut sess = Session::new()
                .map_err(|e| TransportError::Ssh(format!("session new: {e}")))?;
            sess.set_tcp_stream(tcp);
            sess.handshake()
                .map_err(|e| TransportError::Ssh(format!("handshake: {e}")))?;
            // Verify the server's host key against the pinned known_hosts
            // entry before we send any credentials.
            check_host_key(&sess, &host, port, &known_hosts)?;
            // `health_check` is public-key only by design; the password
            // auth path is exercised in `push` (it's the one with the
            // user-facing call site).
            #[cfg(unix)]
            {
                sess.userauth_pubkey_memory(&user, None, &pem, None)
                    .map_err(|e| TransportError::Ssh(format!("pubkey auth: {e}")))?;
                // If we got here, the connection + auth work. health_check
                // is a liveness probe, not a full round-trip — no file push.
                Ok(())
            }
            #[cfg(not(unix))]
            {
                Err(TransportError::Ssh(
                    "pubkey-in-memory auth requires unix in v1; Windows builds are not supported yet".into(),
                ))
            }
        })
        .await
        .map_err(|e| TransportError::Ssh(format!("join: {e}")))?
    }
}

/// Check the server's host key against the configured known_hosts file.
/// Returns `Ok(())` if the server's key matches the pinned entry, or one
/// of:
/// - `TransportError::HostKeyUnknown` — server's key is not in known_hosts
///   (carries the SHA256 fingerprint so the caller can build a TOFU prompt)
/// - `TransportError::HostKeyMismatch` — server's key disagrees with the
///   pinned entry (hard failure; possible MITM)
/// - `TransportError::Ssh(_)` — internal error (read failure, missing
///   host key from server, etc.)
fn check_host_key(
    sess: &ssh2::Session,
    host: &str,
    port: u16,
    known_hosts_path: &Path,
) -> Result<(), TransportError> {
    use ssh2::KnownHostFileKind;

    let mut kh = sess
        .known_hosts()
        .map_err(|e| TransportError::Ssh(format!("known_hosts init: {e}")))?;

    // read_file distinguishes:
    //   Ok(_)              → file loaded (or was empty but exists)
    //   Err + !exists()    → never pinned yet → fall through to NotFound (expected)
    //   Err + exists()     → file IS there but unreadable → user-actionable error
    match kh.read_file(known_hosts_path, KnownHostFileKind::OpenSSH) {
        Ok(_) => {}
        Err(_) if !known_hosts_path.exists() => {}
        Err(e) => {
            return Err(TransportError::Ssh(format!(
                "known_hosts at {} exists but could not be read: {e}",
                known_hosts_path.display()
            )));
        }
    }

    let (key, _key_type) = sess
        .host_key()
        .ok_or_else(|| TransportError::Ssh("server presented no host key".into()))?;

    // Capture the SHA256 fingerprint in ssh-keygen -lf format:
    // "SHA256:" + base64(no padding) of the SHA256 hash of the key bytes.
    let fingerprint = match sess.host_key_hash(ssh2::HashType::Sha256) {
        Some(hash_bytes) => {
            use base64::Engine as _;
            format!(
                "SHA256:{}",
                base64::engine::general_purpose::STANDARD_NO_PAD.encode(hash_bytes)
            )
        }
        None => String::from("<fingerprint unavailable>"),
    };

    let result = kh.check_port(host, port, key);
    map_check_result(result, host, port, &fingerprint)
}

/// Pure mapping from ssh2::CheckResult to TransportError. Extracted as a
/// pure function so it can be table-tested without a real Session.
fn map_check_result(
    r: ssh2::CheckResult,
    host: &str,
    port: u16,
    fingerprint: &str,
) -> Result<(), TransportError> {
    match r {
        ssh2::CheckResult::Match => Ok(()),
        ssh2::CheckResult::NotFound => Err(TransportError::HostKeyUnknown {
            host: host.to_string(),
            port,
            fingerprint: fingerprint.to_string(),
        }),
        ssh2::CheckResult::Mismatch => Err(TransportError::HostKeyMismatch {
            host: host.to_string(),
            port,
        }),
        ssh2::CheckResult::Failure => Err(TransportError::Ssh(format!(
            "host key check failed for {host}:{port}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SshAuth;

    #[test]
    fn from_config_ssh_dispatch() {
        // Verify `from_config` returns an `SshTransport` (identity via `name()`)
        // when fed the SSH variant of `TransportConfig`.
        let cfg = TransportConfig::Ssh {
            host: "vm.example.com".into(),
            port: 22,
            user: "pedro".into(),
            auth: SshAuth::PublicKey {
                path: PathBuf::from("/tmp/key"),
            },
            remote_path: PathBuf::from("/home/pedro/inbox/"),
            known_hosts: PathBuf::from("/tmp/known_hosts"),
        };
        let t = from_config(&cfg).unwrap();
        assert_eq!(t.name(), "ssh");
    }

    #[test]
    fn name_returns_static_str() {
        // `name()` is called by the frontend for diagnostics — must be a
        // stable literal that doesn't allocate.
        let t = SshTransport::new(
            "x".into(),
            22,
            "u".into(),
            SshAuth::PublicKey {
                path: PathBuf::from("/k"),
            },
            PathBuf::from("/r/"),
            PathBuf::from("/tmp/known_hosts"),
        );
        assert_eq!(t.name(), "ssh");
    }

    #[test]
    fn constructor_preserves_fields() {
        let t = SshTransport::new(
            "vm.example.com".into(),
            2222,
            "pedro".into(),
            SshAuth::Password {
                env_var: "SSH_PASSWORD".into(),
            },
            PathBuf::from("/home/pedro/inbox/"),
            PathBuf::from("/tmp/known_hosts"),
        );
        assert_eq!(t.host, "vm.example.com");
        assert_eq!(t.port, 2222);
        assert_eq!(t.user, "pedro");
        assert!(matches!(t.auth, SshAuth::Password { .. }));
        assert_eq!(t.remote_path, PathBuf::from("/home/pedro/inbox/"));
    }

    #[test]
    fn health_check_error_maps_without_keychain_entry() {
        // Agent's Linux host has no entry in the `com.pedrotramontin.trail`
        // macOS Keychain service. The test exercises the
        // `load_private_key_pem()` error-mapping path: the result must
        // be `Err(...)` (any variant proving the mapping is wired up).
        let t = SshTransport::new(
            "vm.example.com".into(),
            22,
            "pedro".into(),
            SshAuth::PublicKey {
                path: PathBuf::from("/k"),
            },
            PathBuf::from("/home/pedro/inbox/"),
            PathBuf::from("/tmp/known_hosts"),
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(t.health_check());
        assert!(
            matches!(
                result,
                Err(TransportError::Ssh(_)) | Err(TransportError::Io(_))
            ),
            "expected Ssh or Io error when keychain has no entry, got: {result:?}"
        );
    }

    /// Thread 5: pure mapping from ssh2::CheckResult to TransportError
    /// must produce the right variant for each input. This is the
    /// seam test for the whole host-key-verification flow.
    #[test]
    fn map_check_result_returns_expected_variant_per_arm() {
        // Match → Ok
        assert!(
            map_check_result(ssh2::CheckResult::Match, "vm.example.com", 22, "SHA256:abc").is_ok()
        );

        // NotFound → HostKeyUnknown carrying host/port/fingerprint
        match map_check_result(
            ssh2::CheckResult::NotFound,
            "vm.example.com",
            22,
            "SHA256:abc",
        ) {
            Err(TransportError::HostKeyUnknown {
                host,
                port,
                fingerprint,
            }) => {
                assert_eq!(host, "vm.example.com");
                assert_eq!(port, 22);
                assert_eq!(fingerprint, "SHA256:abc");
            }
            other => panic!("expected HostKeyUnknown, got {other:?}"),
        }

        // Mismatch → HostKeyMismatch carrying host/port (no fingerprint;
        // mismatch means we already HAVE a pinned key, no need to show one)
        match map_check_result(
            ssh2::CheckResult::Mismatch,
            "vm.example.com",
            22,
            "SHA256:abc",
        ) {
            Err(TransportError::HostKeyMismatch { host, port }) => {
                assert_eq!(host, "vm.example.com");
                assert_eq!(port, 22);
            }
            other => panic!("expected HostKeyMismatch, got {other:?}"),
        }

        // Failure → TransportError::Ssh (generic; carries a message)
        match map_check_result(
            ssh2::CheckResult::Failure,
            "vm.example.com",
            22,
            "SHA256:abc",
        ) {
            Err(TransportError::Ssh(msg)) => {
                assert!(msg.contains("vm.example.com:22"));
            }
            other => panic!("expected TransportError::Ssh, got {other:?}"),
        }
    }
}
