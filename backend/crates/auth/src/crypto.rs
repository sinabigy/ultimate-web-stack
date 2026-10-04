//! AES-256-GCM encryption for identity-provider tokens stored server-side (e.g. the ID token
//! kept for RP-initiated logout). Format: `version(1) || nonce(12) || ciphertext+tag`.
//! The associated data binds a ciphertext to its purpose and owner (e.g. session id), so a
//! ciphertext copied to another row fails to decrypt.

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit, Payload},
};
use base64::{Engine, engine::general_purpose::STANDARD};

const VERSION: u8 = 1;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CryptoError {
    #[error("token encryption key must be 32 bytes, base64-encoded")]
    BadKey,
    #[error("ciphertext is malformed or was not produced for this context")]
    Decrypt,
}

#[derive(Clone)]
pub struct TokenCipher {
    cipher: Aes256Gcm,
}

impl std::fmt::Debug for TokenCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenCipher(<key redacted>)")
    }
}

impl TokenCipher {
    pub fn from_base64_key(key_b64: &str) -> Result<Self, CryptoError> {
        let key = STANDARD.decode(key_b64.trim()).map_err(|_| CryptoError::BadKey)?;
        Self::from_key(&key)
    }

    pub fn from_key(key: &[u8]) -> Result<Self, CryptoError> {
        if key.len() != 32 {
            return Err(CryptoError::BadKey);
        }
        Ok(Self { cipher: Aes256Gcm::new_from_slice(key).map_err(|_| CryptoError::BadKey)? })
    }

    /// Ephemeral key for development when none is configured (tokens do not survive restarts).
    pub fn ephemeral() -> Self {
        Self::from_key(&crate::tokens::random_bytes::<32>()).unwrap_or_else(|_| unreachable!("32-byte key"))
    }

    pub fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> Vec<u8> {
        let nonce_bytes = crate::tokens::random_bytes::<12>();
        let nonce = Nonce::from(nonce_bytes);
        // Encryption with a valid key and in-memory buffers cannot fail.
        let ct = self.cipher.encrypt(&nonce, Payload { msg: plaintext, aad }).unwrap_or_default();
        let mut out = Vec::with_capacity(1 + 12 + ct.len());
        out.push(VERSION);
        out.extend_from_slice(&nonce_bytes);
        out.extend_from_slice(&ct);
        out
    }

    pub fn decrypt(&self, data: &[u8], aad: &[u8]) -> Result<Vec<u8>, CryptoError> {
        if data.len() < 1 + 12 + 16 || data[0] != VERSION {
            return Err(CryptoError::Decrypt);
        }
        let nonce_bytes: [u8; 12] = data[1..13].try_into().map_err(|_| CryptoError::Decrypt)?;
        self.cipher
            .decrypt(&Nonce::from(nonce_bytes), Payload { msg: &data[13..], aad })
            .map_err(|_| CryptoError::Decrypt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_tamper_detection() {
        let c = TokenCipher::ephemeral();
        let ct = c.encrypt(b"id-token", b"session:1");
        assert_eq!(c.decrypt(&ct, b"session:1").unwrap_or_default(), b"id-token");
        assert_eq!(c.decrypt(&ct, b"session:2"), Err(CryptoError::Decrypt), "bound to context");
        let mut bad = ct.clone();
        let last = bad.len() - 1;
        bad[last] ^= 1;
        assert_eq!(c.decrypt(&bad, b"session:1"), Err(CryptoError::Decrypt), "tamper detected");
        assert_ne!(c.encrypt(b"x", b""), c.encrypt(b"x", b""), "fresh nonce per message");
        assert_eq!(TokenCipher::ephemeral().decrypt(&ct, b"session:1"), Err(CryptoError::Decrypt), "other key");
    }

    #[test]
    fn key_validation() {
        assert_eq!(TokenCipher::from_base64_key("c2hvcnQ=").map(|_| ()), Err(CryptoError::BadKey));
        let k = STANDARD.encode([7u8; 32]);
        assert!(TokenCipher::from_base64_key(&k).is_ok());
    }
}
