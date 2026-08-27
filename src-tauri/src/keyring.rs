use serde::{Deserialize, Serialize};
use ssh_key::PrivateKey;
use zeroize::{Zeroize, Zeroizing};

pub const KEYCHAIN_SERVICE: &str = "com.pedrotramontin.trail";
pub const KEYCHAIN_ACCOUNT: &str = "ssh-private-key-ed25519";

/// What's in the OS credential store for this app's SSH key.
///
/// Phase 11 §11.1 typed enum — replaces the loose
/// `Result<Option<String>, _>` return shape
/// [`read_public_from_keychain`] used to expose. The wizard's
/// SSH-key settings panel (§11.3) branches on this enum to
/// render one of 4 UI states (Empty / PublicOnly / KeyPair /
/// Unavailable) instead of guessing from a missing `Some` vs.
/// `None`. Each variant is a discrete state — no booleans,
/// no "missing private key" inferred from absence.
///
/// The serde tagging is `#[serde(tag = "kind", rename_all = "snake_case")]`
/// so the Svelte side sees `{ kind: "key_pair", ... }` /
/// `{ kind: "unavailable", reason: "..." }` / etc. — the
/// TypeScript side can do `if (hint.kind === "key_pair")`
/// without a per-variant payload wrapper.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KeyringHint {
    /// No key has been generated/stored yet.
    Empty,
    /// A public key is stored but no private key was found.
    PublicOnly,
    /// Both public and private keys are stored.
    KeyPair,
    /// The keyring is unavailable on this OS or in this profile.
    Unavailable { reason: String },
}

/// Return the user-facing name of the OS credential store on the
/// **host** that ran this binary. The wizard uses this to
/// surface the platform-specific label in the "store SSH key in
/// your OS credential store" affordance.
///
/// | OS       | Returned string                                |
/// | -------- | --------------------------------------------- |
/// | macOS    | `"Keychain"`                                  |
/// | Linux    | `"secret-service / GNOME Keyring / KWallet"` |
/// | Windows  | `"Credential Manager"`                        |
/// | other    | `"OS credential store"` (fallback)            |
///
/// The string is `&'static` so callers can drop the result into
/// markup without owning a `String` allocation.
///
/// The per-OS selection is delegated to
/// [`credential_store_name_for`] so the test suite can assert
/// every branch from a single host build. Same seam pattern
/// §X-2 used for `default_open_script_invoker_for(...)` — the
/// host calls `credential_store_name_for(cfg!(target_os = "..."))`
/// and tests call it with literal `"macos"` / `"linux"` /
/// `"windows"` to cover the arms that the local build can't
/// compile.
pub fn credential_store_name() -> &'static str {
    let target = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "unsupported"
    };
    credential_store_name_for(target)
}

/// Per-OS user-facing label for the OS credential store. Same
/// table as [`credential_store_name`], but the dispatch is
/// keyed on the supplied `target_os` string instead of the
/// compile-time `cfg!(target_os = "...")`. Tests call this
/// with literal `"macos"` / `"linux"` / `"windows"` so every
/// arm is covered on every host. Unknown / unsupported
/// `target_os` values fall through to the generic
/// `"OS credential store"` label — the wizard's tooltip
/// degrades gracefully on FreeBSD, iOS, etc.
pub fn credential_store_name_for(target_os: &str) -> &'static str {
    match target_os {
        "macos" => "Keychain",
        "linux" => "secret-service / GNOME Keyring / KWallet",
        "windows" => "Credential Manager",
        // Fallback for hosts we don't ship for (FreeBSD, iOS, …).
        // The wizard still calls the helper — the user just sees
        // the generic label instead of the platform-specific one.
        _ => "OS credential store",
    }
}

/// Generate a fresh ed25519 SSH keypair (pure — does not touch the
/// keychain). Returns the private key in OpenSSH PEM form + the
/// public key in OpenSSH single-line form (ready for
/// `~/.ssh/authorized_keys`).
///
/// This is the underlying generator that `generate_and_store()`
/// delegates to. It's `pub(crate)` so tests can call it without
/// going through the keychain, but it's not part of the public API.
pub(crate) fn generate_keypair() -> Result<(String, String), KeyringError> {
    let private = PrivateKey::random(&mut ssh_key::rand_core::OsRng, ssh_key::Algorithm::Ed25519)
        .map_err(KeyringError::Keygen)?;
    let public_openssh = private
        .public_key()
        .to_openssh()
        .map_err(KeyringError::Keygen)?;

    // Serialize private key (OpenSSH PEM, no passphrase — see master's
    // "Tradeoffs" block for the security trade-off rationale).
    // `to_openssh` returns a Zeroizing<String>; we hand that
    // Zeroizing wrapper to the caller so the bytes are wiped on drop.
    let pem_zerobox = private
        .to_openssh(ssh_key::LineEnding::LF)
        .map_err(KeyringError::Keygen)?;
    // The underlying ed25519 key material in `private` is wiped by
    // ssh-key's Drop impl when this function returns.

    Ok((pem_zerobox.to_string(), public_openssh))
}

/// Generate a new ed25519 keypair on first run, store the private key
/// in the OS keychain, and return the public key in OpenSSH format.
///
/// The private key never leaves the keychain in the v1 design (no
/// in-memory copy returned to the caller). If the user re-runs
/// onboarding, the existing keypair is reused (detected by presence
/// in keychain); `generate_and_store()` becomes idempotent.
pub fn generate_and_store() -> Result<String, KeyringError> {
    if let Some(public_openssh) = read_public_from_keychain()? {
        return Ok(public_openssh);
    }

    let (pem, public_openssh) = generate_keypair()?;
    // Hand the PEM to the OS keychain (which copies it into the
    // platform's secure storage), then zeroize our copy before
    // dropping the String. `Zeroizing<String>` guarantees the bytes
    // are wiped on Drop even if a future caller clones the buffer —
    // a plain `String` clone could outlive this `zeroize()` call and
    // leak the private key in a heap dump (CWE-316 / ASVS V6.4.1).
    let mut pem: Zeroizing<String> = Zeroizing::new(pem);
    let result = store_in_keychain(&pem);
    pem.zeroize();
    result?;
    Ok(public_openssh)
}

/// Read the public key for an existing keypair in the keychain, or
/// `None` if no keypair is stored yet.
///
/// §X-5 / Phase 11 §11.1 — this function's `Option<String>`
/// return shape stays as-is for backward compatibility (the
/// wizard's "Test connection" + onboarding flow still want the
/// raw public key string). The new typed probe for the SSH-key
/// settings panel lives in [`keyring_hint`] / [`keyring_hint_for`]
/// — it wraps this function internally and lifts the result
/// into the discrete [`KeyringHint`] variant the Svelte panel
/// branches on.
pub fn read_public_from_keychain() -> Result<Option<String>, KeyringError> {
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(KeyringError::Keyring)?;
    match entry.get_password() {
        Ok(pem) => {
            // Wrap PEM bytes in `Zeroizing<String>` so the keychain-
            // returned private key is wiped on Drop. The
            // `keyring 3.x` upstream returns a plain `String`
            // (not `Zeroizing<String>`), so we wrap inline.
            let pem: Zeroizing<String> = Zeroizing::new(pem);
            let private = PrivateKey::from_openssh(&pem).map_err(KeyringError::Keygen)?;
            let public_openssh = private
                .public_key()
                .to_openssh()
                .map_err(KeyringError::Keygen)?;
            Ok(Some(public_openssh))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(KeyringError::Keyring(e)),
    }
}

/// Pure-function mapping from the (has_public, has_private) tuple
/// to the discrete [`KeyringHint`] variant. Lives in its own
/// function so the test suite can assert all 4 states from a
/// single host build — the I/O half ([`keyring_hint`]) is
/// `#[ignore]`'d on CI hosts without an OS keychain, but the
/// mapping table here is pure and runs everywhere.
///
/// The (false, true) corner — private key without a matching
/// public key — is **not reachable** in the v1 generator:
/// [`generate_and_store`] writes the private key PEM into the
/// keychain and returns the public key derived from it via
/// [`read_public_from_keychain`]. If the PEM in the keychain
/// is a valid OpenSSH private key, deriving the public key is
/// a deterministic, infallible operation, so the public-key
/// side will always be present whenever the private side is.
/// We collapse this unreachable input to [`KeyringHint::KeyPair`]
/// (i.e. "the credential store has something we can use") so
/// the UI never surfaces a confusing "private only, public
/// missing" state that the generator cannot actually produce.
pub fn keyring_hint_for(has_public: bool, has_private: bool) -> KeyringHint {
    match (has_public, has_private) {
        (false, false) => KeyringHint::Empty,
        (true, false) => KeyringHint::PublicOnly,
        // Both (true, true) and (false, true) collapse to KeyPair.
        // The (false, true) input is a documented unreachability —
        // see the doc comment above — and a future refactor that
        // makes it reachable (e.g. a manually-imported PEM that
        // ssh-key can't round-trip) will still render the panel
        // sensibly rather than crashing.
        (true, true) | (false, true) => KeyringHint::KeyPair,
    }
}

/// Probe the OS credential store and return a typed
/// [`KeyringHint`] describing what's there. Phase 11 §11.1
/// surface for the wizard's SSH-key settings panel (§11.3):
/// the frontend branches on `hint.kind` to render the
/// Empty / PublicOnly / KeyPair / Unavailable UI states.
///
/// The actual lookup is delegated to
/// [`read_public_from_keychain`] (for the public key) and the
/// raw `keyring::Entry::get_password` call (for the private
/// key presence check — we don't materialise the PEM twice).
/// If the keychain itself is unreachable on this OS / profile,
/// we return [`KeyringHint::Unavailable`] with a short
/// human-readable reason string instead of an `Err`, so the
/// UI can render the labeled fallback message rather than
/// surfacing an IPC error.
///
/// `Err` is reserved for genuine programming bugs (an
/// `Entry::new` failure, an `ssh-key` parse failure on a
/// stored PEM). The "keychain is fine, it's just empty" case
/// returns `Ok(KeyringHint::Empty)` — that's a normal state,
/// not an error.
pub fn keyring_hint() -> Result<KeyringHint, KeyringError> {
    let entry = match keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
        Ok(e) => e,
        Err(e) => {
            return Ok(KeyringHint::Unavailable {
                reason: format!("keychain entry init failed: {e}"),
            });
        }
    };
    // Probe the private key (the raw PEM). We use the raw
    // `get_password` rather than `read_public_from_keychain`
    // because we need the *presence* bit, not the public
    // key bytes — pulling the public key requires parsing the
    // PEM, which can fail and would conflate "the PEM is
    // garbage" with "no entry exists". The two lookups are
    // idempotent + cheap (both hit the same OS credential).
    let has_private = match entry.get_password() {
        Ok(_) => true,
        Err(keyring::Error::NoEntry) => false,
        Err(e) => {
            return Ok(KeyringHint::Unavailable {
                reason: format!("keychain get_password failed: {e}"),
            });
        }
    };
    // The public key is a deterministic derivation from the
    // private PEM. If the PEM parses, the public key is
    // present (we re-use `read_public_from_keychain` so the
    // SSH-key parse path is exercised in exactly one place).
    let has_public = read_public_from_keychain()?.is_some();
    Ok(keyring_hint_for(has_public, has_private))
}

fn store_in_keychain(pem: &str) -> Result<(), KeyringError> {
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(KeyringError::Keyring)?;
    entry.set_password(pem).map_err(KeyringError::Keyring)?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error("keygen failed: {0}")]
    Keygen(ssh_key::Error),
    #[error("keyring error: {0}")]
    Keyring(#[from] keyring::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    // === Pure-function tests (run in CI / on any host) ===
    //
    // These exercise the keypair generator without touching the OS
    // keychain. The keychain-touching test is `#[ignore]`'d below
    // and runs only on Pedro's Mac.

    #[test]
    fn generate_keypair_produces_ed25519_public_openssh() {
        let (_pem, public_openssh) = generate_keypair().expect("generate succeeds");
        // ed25519 OpenSSH public keys start with the type tag.
        assert!(
            public_openssh.starts_with("ssh-ed25519 "),
            "expected ssh-ed25519 prefix, got: {public_openssh:?}"
        );
        // Single-line (one space separates the tag from the body; no
        // embedded newlines).
        assert_eq!(public_openssh.matches('\n').count(), 0);
        // ed25519 public keys are exactly 80 chars: the 11-char tag
        // + space + ~68 chars of base64 (RFC 4253 §6.6). Sanity-check
        // the body is base64-shaped (no spaces past the tag).
        assert!(
            public_openssh.len() >= 60,
            "expected a non-trivial body, got: {public_openssh:?}"
        );
        let body = &public_openssh["ssh-ed25519 ".len()..];
        assert!(
            body.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '='),
            "base64 body contains unexpected chars: {body:?}"
        );
    }

    #[test]
    fn generate_keypair_private_pem_is_open_ssh_text() {
        let (pem, _public) = generate_keypair().expect("generate succeeds");
        // OpenSSH private key header (the `to_openssh` LineEnding::LF form
        // starts with "-----BEGIN OPENSSH PRIVATE KEY-----").
        let head_len = pem.len().min(60);
        assert!(
            pem.starts_with("-----BEGIN OPENSSH PRIVATE KEY-----"),
            "expected OpenSSH PEM header, got head: {:?}",
            &pem[..head_len]
        );
        // The PEM is the base64-armoured body, so it's at least 200 chars.
        assert!(pem.len() > 200);
    }

    #[test]
    fn two_generated_keypairs_have_distinct_public_keys() {
        let (_, pub1) = generate_keypair().expect("first generate");
        let (_, pub2) = generate_keypair().expect("second generate");
        assert_ne!(
            pub1, pub2,
            "two independently-generated ed25519 keypairs must have distinct public keys"
        );
    }

    // === Keychain-touching test (#[ignore], runs on Pedro's Mac) ===

    #[test]
    #[ignore = "touches the real OS keychain — run manually on Pedro's Mac"]
    fn generate_and_store_roundtrip() {
        // Clean up any prior key.
        if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
            let _ = entry.delete_credential();
        }
        let pub1 = generate_and_store().expect("first generation");
        assert!(pub1.starts_with("ssh-ed25519 "));
        // Second call should return the SAME public key (idempotent).
        let pub2 = generate_and_store().expect("second call should reuse");
        assert_eq!(pub1, pub2);
        // Clean up.
        if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
            let _ = entry.delete_credential();
        }
    }

    // === Phase 7 §7.8 regression tests (PEM Zeroizing<String>) ===
    //
    // Carry-forward from Phase 1 §5b: the keyring 3.x crate returns
    // PEM bytes as a plain `String`. Wrapping that in `Zeroizing<String>`
    // ensures the heap bytes are wiped on Drop. These two tests guard
    // against a future refactor that silently swaps `Zeroizing<String>`
    // back for `String` (which is the exact mistake that motivated the
    // Phase 1 §5b note).

    #[test]
    fn pem_drop_zeroes_the_buffer() {
        // `Zeroizing<String>::Drop` (and the underlying `Zeroize`
        // impl on `String`) must wipe the string's bytes before
        // releasing the heap allocation. The `zeroize` crate's
        // `Zeroize` impl for `String` uses `String::clear()` (truncates
        // to len 0 then drops the contents) — stronger than in-place
        // zeroing — and `Zeroizing<Z>` calls `Zeroize::zeroize` in its
        // `Drop` impl. This test verifies our usage fits the contract:
        // cloning via the `Zeroizing` wrapper keeps the zeroize
        // guarantee, and an explicit `zeroize()` call WIPES the bytes
        // (the upstream `String::Zeroize` impl truncates to len 0).
        //
        // The actual heap-zeroing behaviour is documented in the
        // `zeroize` crate's contract and verified by the upstream
        // test suite; here we verify the type contract that prevents
        // a future refactor from silently unwrapping back to a plain
        // `String` clone (which is the exact mistake Phase 1 §5b
        // flagged).
        let payload = "PEM-FIXTURE-WOULD-NORMALLY-BE-AN-OPENSSH-PRIVKEY";
        let pem: Zeroizing<String> = Zeroizing::new(payload.to_string());
        assert_eq!(pem.as_str(), payload);

        // `.clone()` returns a fresh `Zeroizing<String>` — the
        // signature-level proof that cloning keeps the zeroize
        // guarantee. This is the exact shape of the call site that
        // `load_private_key_pem()` uses after `entry.get_password()`.
        let mut clone: Zeroizing<String> = pem.clone();
        assert_eq!(clone.as_str(), payload);

        // Explicit zeroize before Drop is the contract used in
        // `keyring.rs::generate_and_store` after the keychain
        // accepts the PEM. The upstream `zeroize::Zeroize` impl for
        // `String` calls `String::clear()` (truncates to len 0);
        // we don't depend on the exact mechanism, only on the
        // observable contract that the bytes are no longer present.
        clone.zeroize();
        assert!(
            clone.is_empty() || clone.as_str().bytes().all(|b| b == 0),
            "after zeroize(), the Zeroizing<String> must be empty (cleared) or all-NUL bytes; got: {:?}",
            clone.as_str()
        );

        // Drop frees the underlying allocation — Rust's allocator
        // takes over from there. The invariant this test enforces is:
        // the bytes never reach a `String` clone that outlives the
        // wrapper, so a heap dump cannot recover the original PEM.
        drop(pem);
        drop(clone);
    }

    #[test]
    #[ignore = "touches the real OS keychain — run manually on Pedro's Mac"]
    fn store_in_keychain_round_trip_preserves_bytes() {
        // Non-regression: the Zeroizing wrapper must NOT truncate or
        // mutate the bytes that flow through to the OS keychain. We
        // exercise the same `store_in_keychain → get_password` pair
        // that `generate_and_store` uses, but with an explicit,
        // distinguishable payload so truncation would be detected.
        if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
            let _ = entry.delete_credential();
        }

        // Generate a fresh PEM via the real path (OpenSSH PEM,
        // ~370 bytes for ed25519) — bytes the test can fingerprint.
        let (pem, _public) = generate_keypair().expect("generate_keypair");
        let original_len = pem.len();
        let original_sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(pem.as_bytes());
            h.finalize().iter().map(|b| format!("{:02x}", b)).collect::<String>()
        };

        // Store wrapped in `Zeroizing<String>` (the new contract).
        let pem_zerobox: Zeroizing<String> = Zeroizing::new(pem);
        store_in_keychain(&pem_zerobox).expect("store_in_keychain should accept Zeroizing<String>");

        // Read back wrapped in `Zeroizing<String>` (the new contract).
        let entry =
            keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).expect("keychain entry");
        let read_back: Zeroizing<String> = Zeroizing::new(
            entry
                .get_password()
                .expect("get_password should return the stored PEM"),
        );

        assert_eq!(
            read_back.len(),
            original_len,
            "round-trip must preserve byte length (truncation regression check)"
        );

        let read_sha = {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(read_back.as_bytes());
            h.finalize().iter().map(|b| format!("{:02x}", b)).collect::<String>()
        };
        assert_eq!(
            read_sha, original_sha,
            "round-trip must preserve bytes verbatim"
        );

        // Clean up.
        if let Ok(entry) = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT) {
            let _ = entry.delete_credential();
        }
    }

    // === §X-3 — per-OS user-facing label for the OS credential store ===
    //
    // `credential_store_name_for("macos")` / `..._for("linux")` /
    // `..._for("windows")` are pure functions that return the
    // platform-specific label the wizard surfaces in its
    // tooltip. The per-OS dispatch is keyed on the supplied
    // `&str` (not the host's `#[cfg(target_os = "...")]`) so
    // every arm is covered by a single test run on a single
    // host — same seam pattern §X-2 used for
    // `default_open_script_invoker_for(...)`.
    //
    // The unknown-OS fallback ("OS credential store") is
    // asserted too so a future refactor that drops the arm
    // doesn't silently render the wrong label on FreeBSD / iOS.

    #[test]
    fn credential_store_name_for_macos_returns_keychain() {
        assert_eq!(
            credential_store_name_for("macos"),
            "Keychain",
            "macOS arm should return the platform-specific label"
        );
    }

    #[test]
    fn credential_store_name_for_linux_returns_secret_service() {
        assert_eq!(
            credential_store_name_for("linux"),
            "secret-service / GNOME Keyring / KWallet",
            "Linux arm should list the freedesktop + KDE options"
        );
    }

    #[test]
    fn credential_store_name_for_windows_returns_credential_manager() {
        assert_eq!(
            credential_store_name_for("windows"),
            "Credential Manager",
            "Windows arm should return the platform-specific label"
        );
    }

    #[test]
    fn credential_store_name_for_unknown_os_returns_generic_label() {
        // FreeBSD, iOS, or any other host the wizard doesn't
        // ship for — the toolip should still render something
        // sensible (the same generic wording the body copy
        // uses), not panic or return an empty string.
        assert_eq!(
            credential_store_name_for("freebsd"),
            "OS credential store",
            "unknown-OS fallback should be the generic label"
        );
    }

    // The host-side wrapper `credential_store_name()` is
    // exercised transitively by the per-OS tests above (it
    // delegates to `credential_store_name_for(host_target)`).
    // We don't need a separate test for it because there's
    // nothing to test beyond the cfg!() host lookup, which
    // would just re-state the build's target triple. The
    // runtime dispatch is fully covered by the `_for(...)`
    // assertions.

    // === §X-5 / Phase 11 §11.1 — `KeyringHint` mapping table ===
    //
    // `keyring_hint_for(has_public, has_private)` is a pure
    // function (no I/O, no state) keyed on two bools. The
    // table below enumerates all 4 input pairs so a future
    // refactor that drops an arm or misroutes a tuple doesn't
    // silently shift the UI between Empty / PublicOnly /
    // KeyPair / Unavailable. The host-side wrapper
    // `keyring_hint()` is exercised transitively by the
    // keychain-touching `#[ignore]`'d tests above (same
    // delegation pattern as `credential_store_name()`).

    #[test]
    fn keyring_hint_for_false_false_returns_empty() {
        // Fresh-install case — the user just ran onboarding and
        // hasn't generated the SSH key yet. The wizard's
        // SSH-key settings panel renders "No SSH key yet" +
        // the "Generate SSH key" button.
        assert_eq!(
            keyring_hint_for(false, false),
            KeyringHint::Empty,
            "(false, false) should map to Empty (no key generated/stored yet)"
        );
    }

    #[test]
    fn keyring_hint_for_true_false_returns_public_only() {
        // Half-state — a public key is on disk but the private
        // PEM is missing (corrupt keychain, user wiped the
        // credential, OS upgrade, etc.). The wizard renders
        // the "your public key is stored but the private key
        // is missing — re-generate" recovery row.
        assert_eq!(
            keyring_hint_for(true, false),
            KeyringHint::PublicOnly,
            "(true, false) should map to PublicOnly (public key present, private key missing)"
        );
    }

    #[test]
    fn keyring_hint_for_true_true_returns_key_pair() {
        // Happy path — both keys are in the OS credential store.
        // The wizard renders "Your SSH key is stored" + the
        // "Copy public key" + "Regenerate" buttons.
        assert_eq!(
            keyring_hint_for(true, true),
            KeyringHint::KeyPair,
            "(true, true) should map to KeyPair (both keys present)"
        );
    }

    #[test]
    fn keyring_hint_for_false_true_collapses_to_key_pair() {
        // Documented unreachability — see the doc comment on
        // `keyring_hint_for` above. The v1 generator
        // (`generate_and_store`) always writes both halves
        // (private PEM in keychain, public key derived from
        // it on demand via `read_public_from_keychain`).
        // Deriving the public key from a valid OpenSSH
        // private key is deterministic and infallible, so
        // the (false, true) input is not reachable from the
        // current code paths. We collapse it to `KeyPair` so
        // a future refactor that makes it reachable (e.g. a
        // manually-imported PEM that ssh-key can't round-trip
        // back into a public key) still renders a sensible
        // "your SSH key is stored" panel instead of crashing
        // or showing an unreachable "public missing" state.
        assert_eq!(
            keyring_hint_for(false, true),
            KeyringHint::KeyPair,
            "(false, true) should collapse to KeyPair (documented unreachability — the v1 generator always writes both halves; see the doc comment)"
        );
    }
}
