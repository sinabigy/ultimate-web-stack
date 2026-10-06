//! Opaque random tokens. 256 bits from the OS-seeded CSPRNG, base64url encoded.
//! Only SHA-256 digests are stored; tokens are high-entropy so a fast hash is appropriate
//! (slow password hashes protect *low*-entropy secrets).

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::fill(&mut b);
    b
}

/// A new 256-bit token, URL-safe.
pub fn new_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

pub fn sha256(data: &[u8]) -> Vec<u8> {
    Sha256::digest(data).to_vec()
}

/// Hash of a token as stored in the database.
pub fn token_hash(token: &str) -> Vec<u8> {
    sha256(token.as_bytes())
}

/// Constant-time equality for secrets (e.g. CSRF token comparison).
pub fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && bool::from(a.as_bytes().ct_eq(b.as_bytes()))
}

/// Tokens arriving from clients must look like ours before we spend a DB lookup on them.
pub fn plausible_token(t: &str) -> bool {
    t.len() == 43 && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_wellformed_and_hashed() {
        let (a, b) = (new_token(), new_token());
        assert_ne!(a, b);
        assert!(plausible_token(&a));
        assert_eq!(token_hash(&a).len(), 32);
        assert_ne!(token_hash(&a), token_hash(&b));
        assert!(!plausible_token("short"));
        assert!(!plausible_token(&format!("{}!", &a[..42])));
    }

    /// Known-answer vectors (computed independently with Python's `base64`/`hashlib`) for the
    /// two engines this project persists data with: tokens, PKCE challenges and pagination cursors
    /// (URL-safe, no padding) and the token-encryption key (standard, padded). A base64 crate
    /// upgrade must not change these, or stored tokens, cursors and keys stop matching.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn base64_encodings_are_stable_known_answers() {
        use base64::{
            Engine,
            engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
        };
        let bytes: Vec<u8> = (0u8..32).collect();
        assert_eq!(URL_SAFE_NO_PAD.encode(&bytes), "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8");
        assert_eq!(STANDARD.encode(&bytes), "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=");
        assert_eq!(URL_SAFE_NO_PAD.encode([0xfb, 0xff, 0xbf]), "-_-_");
        assert_eq!(STANDARD.encode([0xfb, 0xff, 0xbf]), "+/+/");
        // PKCE S256 challenge shape: base64url(sha256(verifier)), no padding.
        assert_eq!(URL_SAFE_NO_PAD.encode(sha256(b"abc")), "ungWv48Bz-pBQUDeXa4iI7ADYaOWF3qctBD_YfIAFa0");
        assert_eq!(STANDARD.decode("c2hvcnQ=").unwrap(), b"short");
        assert_eq!(URL_SAFE_NO_PAD.decode("-_-_").unwrap(), [0xfb, 0xff, 0xbf]);
        // Each engine rejects the other's alphabet and padding rules.
        assert!(URL_SAFE_NO_PAD.decode("c2hvcnQ=").is_err(), "padding is rejected by the no-pad engine");
        assert!(URL_SAFE_NO_PAD.decode("+/+/").is_err(), "standard alphabet is rejected by the URL-safe engine");
        assert!(STANDARD.decode("-_-_").is_err(), "URL-safe alphabet is rejected by the standard engine");
    }

    #[test]
    fn constant_time_compare() {
        assert!(ct_eq("abc", "abc"));
        assert!(!ct_eq("abc", "abd"));
        assert!(!ct_eq("abc", "abcd"));
    }
}
