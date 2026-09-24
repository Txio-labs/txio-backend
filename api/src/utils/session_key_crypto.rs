use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, Nonce};
use base64::{engine::general_purpose::STANDARD, Engine};
use sha2::{Digest, Sha256};

use crate::utils::error::AppError;

/// Encrypts an ephemeral automation signer's private key at rest, so a
/// session key's raw private key is never stored in plaintext even though
/// the backend needs to decrypt and use it unattended (see the Phase 5 plan:
/// scheduled execution has no human to prompt for a signature, so the
/// backend — not the frontend — must hold this key; the blast radius of a
/// leak is capped by what each session key is scoped/capped to, unlike the
/// user's real wallet key, which Txio never sees at all).
///
/// SESSION_KEY_ENCRYPTION_KEY is hashed with SHA-256 to derive a fixed
/// 32-byte AES-256 key regardless of the configured secret's length (the
/// config loader already requires >=32 characters, but AES-256-GCM needs
/// exactly 32 bytes).
fn derive_key(secret: &str) -> Key<Aes256Gcm> {
    let digest = Sha256::digest(secret.as_bytes());
    *Key::<Aes256Gcm>::from_slice(&digest)
}

/// Returns `base64(nonce || ciphertext)` — the nonce is stored alongside the
/// ciphertext (standard AES-GCM practice) since it isn't secret, only
/// required to be unique per encryption, which `OsRng` guarantees.
pub fn encrypt(secret_key: &str, plaintext: &str) -> Result<String, AppError> {
    let key = derive_key(secret_key);
    let cipher = Aes256Gcm::new(&key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);

    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| AppError::InternalError("Failed to encrypt session key".into()))?;

    let mut combined = nonce.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(combined))
}

pub fn decrypt(secret_key: &str, encoded: &str) -> Result<String, AppError> {
    let key = derive_key(secret_key);
    let cipher = Aes256Gcm::new(&key);

    let combined = STANDARD
        .decode(encoded)
        .map_err(|_| AppError::InternalError("Corrupt session key ciphertext".into()))?;

    if combined.len() < 12 {
        return Err(AppError::InternalError("Corrupt session key ciphertext".into()));
    }
    let (nonce_bytes, ciphertext) = combined.split_at(12);
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| AppError::InternalError("Failed to decrypt session key".into()))?;

    String::from_utf8(plaintext).map_err(|_| AppError::InternalError("Corrupt session key plaintext".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_a_secret() {
        let key = "a-32-character-or-longer-secret!";
        let encrypted = encrypt(key, "0xdeadbeef-private-key").unwrap();
        assert_ne!(encrypted, "0xdeadbeef-private-key");
        assert_eq!(decrypt(key, &encrypted).unwrap(), "0xdeadbeef-private-key");
    }

    #[test]
    fn wrong_key_fails_to_decrypt() {
        let encrypted = encrypt("a-32-character-or-longer-secret!", "secret-value").unwrap();
        assert!(decrypt("a-different-32-character-secret", &encrypted).is_err());
    }

    #[test]
    fn each_encryption_uses_a_fresh_nonce() {
        let key = "a-32-character-or-longer-secret!";
        let a = encrypt(key, "same-plaintext").unwrap();
        let b = encrypt(key, "same-plaintext").unwrap();
        assert_ne!(a, b, "identical plaintext must not produce identical ciphertext");
    }
}
