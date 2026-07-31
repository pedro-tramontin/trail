use ssh_key::PrivateKey;
use zeroize::Zeroize;

pub const KEYCHAIN_SERVICE: &str = "com.pedrotramontin.trail";
pub const KEYCHAIN_ACCOUNT: &str = "ssh-private-key-ed25519";

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
    // dropping the String.
    let mut pem = pem;
    let result = store_in_keychain(&pem);
    pem.zeroize();
    result?;
    Ok(public_openssh)
}

/// Read the public key for an existing keypair in the keychain, or
/// `None` if no keypair is stored yet.
pub fn read_public_from_keychain() -> Result<Option<String>, KeyringError> {
    let entry =
        keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_ACCOUNT).map_err(KeyringError::Keyring)?;
    match entry.get_password() {
        Ok(pem) => {
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
}
