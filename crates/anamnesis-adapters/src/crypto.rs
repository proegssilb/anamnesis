//! [`AesGcmTokenCipher`]: encrypts a project sync config's external-tracker
//! token at rest (issues #40/#41) — the one place in this codebase that
//! actually encrypts anything, as opposed to `anamnesis-web::config::Secret`,
//! which only redacts `Debug` output for values that are never persisted.

use aes_gcm::aead::rand_core::RngCore;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};

use anamnesis_app::{AppError, TokenCipher};

/// Bytes of nonce prepended to every ciphertext — AES-GCM's standard
/// 96-bit/12-byte nonce size.
const NONCE_LEN: usize = 12;

/// AES-256-GCM, keyed once at startup from `ANAMNESIS_SYNC_ENCRYPTION_KEY`
/// (validated as exactly 32 bytes by `anamnesis-web::config` before this is
/// constructed — this type trusts the key it's given). [`Self::encrypt`]
/// generates a fresh random nonce per call and prepends it to the
/// ciphertext (`nonce || ciphertext`); [`Self::decrypt`] splits it back off.
/// GCM's authentication tag means a tampered or wrong-key ciphertext fails
/// to decrypt rather than silently returning garbage.
pub struct AesGcmTokenCipher {
    cipher: Aes256Gcm,
}

impl AesGcmTokenCipher {
    pub fn new(key: &[u8; 32]) -> Self {
        Self {
            cipher: Aes256Gcm::new(&Key::<Aes256Gcm>::from(*key)),
        }
    }
}

// Hand-written so the expanded key material can never be printed by
// accident, mirroring `anamnesis_web::config::Secret`'s own reasoning.
impl std::fmt::Debug for AesGcmTokenCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AesGcmTokenCipher(<redacted>)")
    }
}

impl TokenCipher for AesGcmTokenCipher {
    fn encrypt(&self, plaintext: &str) -> Result<Vec<u8>, AppError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = Nonce::from(nonce_bytes);
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_bytes())
            .map_err(|_| AppError::Crypto("failed to encrypt token".to_string()))?;
        let mut out = Vec::with_capacity(NONCE_LEN + ciphertext.len());
        out.extend_from_slice(&nonce_bytes);
        out.extend(ciphertext);
        Ok(out)
    }

    fn decrypt(&self, ciphertext: &[u8]) -> Result<String, AppError> {
        if ciphertext.len() < NONCE_LEN {
            return Err(AppError::Crypto(
                "stored ciphertext is too short to contain a nonce".to_string(),
            ));
        }
        let (nonce_bytes, ct) = ciphertext.split_at(NONCE_LEN);
        let nonce_bytes: [u8; NONCE_LEN] = nonce_bytes
            .try_into()
            .expect("split_at(NONCE_LEN) guarantees exactly NONCE_LEN bytes");
        let nonce = Nonce::from(nonce_bytes);
        let plaintext = self.cipher.decrypt(&nonce, ct).map_err(|_| {
            AppError::Crypto(
                "failed to decrypt token (wrong encryption key, or the ciphertext was tampered with)"
                    .to_string(),
            )
        })?;
        String::from_utf8(plaintext)
            .map_err(|e| AppError::Crypto(format!("decrypted token was not valid UTF-8: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cipher() -> AesGcmTokenCipher {
        AesGcmTokenCipher::new(&[7u8; 32])
    }

    #[test]
    fn round_trips_a_plaintext_token() {
        let c = cipher();
        let ciphertext = c.encrypt("ghp_supersecret").unwrap();
        assert_eq!(c.decrypt(&ciphertext).unwrap(), "ghp_supersecret");
    }

    #[test]
    fn encrypting_the_same_plaintext_twice_produces_different_ciphertexts() {
        let c = cipher();
        let a = c.encrypt("same token").unwrap();
        let b = c.encrypt("same token").unwrap();
        assert_ne!(a, b, "each call must use a fresh random nonce");
        assert_eq!(c.decrypt(&a).unwrap(), "same token");
        assert_eq!(c.decrypt(&b).unwrap(), "same token");
    }

    #[test]
    fn a_tampered_ciphertext_fails_to_decrypt() {
        let c = cipher();
        let mut ciphertext = c.encrypt("ghp_supersecret").unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0xFF;
        assert!(matches!(c.decrypt(&ciphertext), Err(AppError::Crypto(_))));
    }

    #[test]
    fn the_wrong_key_fails_to_decrypt() {
        let ciphertext = cipher().encrypt("ghp_supersecret").unwrap();
        let other = AesGcmTokenCipher::new(&[9u8; 32]);
        assert!(matches!(other.decrypt(&ciphertext), Err(AppError::Crypto(_))));
    }

    #[test]
    fn a_too_short_ciphertext_is_rejected_without_panicking() {
        let c = cipher();
        assert!(matches!(c.decrypt(&[1, 2, 3]), Err(AppError::Crypto(_))));
    }
}
