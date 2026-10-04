//! API keys: `<prefix>_<env>_<key_id>_<secret>`, e.g. `app_live_k3J9…_Q2xv…`.
//!
//! - `prefix` identifies the application (configurable); `env` is `live` or `test` so a test
//!   key can never be confused with production; both make leaked keys easy to scan for.
//! - `key_id` (16 base62 chars) is public and indexed; `secret` (43 base62 chars ≈ 256 bits)
//!   is shown once and stored only as HMAC-SHA256(pepper, secret). The pepper lives outside
//!   the database, so a database leak alone cannot be used to verify guesses offline.

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use subtle::ConstantTimeEq;

const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const ID_LEN: usize = 16;
const SECRET_LEN: usize = 43;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedKey<'a> {
    pub env: &'a str,
    pub key_id: &'a str,
    pub secret: &'a str,
}

/// Base62 string of `len` chars with rejection sampling (no modulo bias).
fn base62(len: usize) -> String {
    let mut out = String::with_capacity(len);
    while out.len() < len {
        for b in crate::tokens::random_bytes::<64>() {
            // 62 * 4 = 248: accept bytes < 248 to keep the distribution uniform
            if b < 248 && out.len() < len {
                out.push(char::from(ALPHABET[usize::from(b % 62)]));
            }
        }
    }
    out
}

pub struct GeneratedKey {
    /// Full key: show to the user exactly once.
    pub plaintext: String,
    pub key_id: String,
    pub secret_hash: Vec<u8>,
}

pub fn generate(prefix: &str, live: bool, pepper: &[u8]) -> GeneratedKey {
    let key_id = base62(ID_LEN);
    let secret = base62(SECRET_LEN);
    let env = if live { "live" } else { "test" };
    GeneratedKey {
        plaintext: format!("{prefix}_{env}_{key_id}_{secret}"),
        secret_hash: hash_secret(pepper, &secret),
        key_id,
    }
}

pub fn hash_secret(pepper: &[u8], secret: &str) -> Vec<u8> {
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(pepper) else { return Vec::new() };
    mac.update(secret.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

pub fn verify_secret(pepper: &[u8], secret: &str, stored_hash: &[u8]) -> bool {
    let h = hash_secret(pepper, secret);
    !h.is_empty() && h.len() == stored_hash.len() && bool::from(h.ct_eq(stored_hash))
}

/// Strictly parse a presented key. Anything malformed is rejected without a DB lookup.
pub fn parse<'a>(prefix: &str, key: &'a str) -> Option<ParsedKey<'a>> {
    let rest = key.strip_prefix(prefix)?.strip_prefix('_')?;
    let (env, rest) = rest.split_once('_')?;
    let (key_id, secret) = rest.split_once('_')?;
    let ok = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_alphanumeric());
    if !(env == "live" || env == "test") || !ok(key_id, ID_LEN) || !ok(secret, SECRET_LEN) {
        return None;
    }
    Some(ParsedKey { env, key_id, secret })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn generate_parse_verify() {
        let pepper = b"pepper";
        let k = generate("app", true, pepper);
        let p = parse("app", &k.plaintext).expect("parses");
        assert_eq!(p.env, "live");
        assert_eq!(p.key_id, k.key_id);
        assert!(verify_secret(pepper, p.secret, &k.secret_hash));
        assert!(!verify_secret(b"other pepper", p.secret, &k.secret_hash));
        let mut tampered = p.secret.to_string();
        let first = if tampered.starts_with('0') { "1" } else { "0" };
        tampered.replace_range(0..1, first);
        assert!(!verify_secret(pepper, &tampered, &k.secret_hash));
        assert!(parse("other", &k.plaintext).is_none(), "wrong prefix");
        assert!(parse("app", &k.plaintext.replace("_live_", "_prod_")).is_none());
        assert_ne!(generate("app", true, pepper).plaintext, k.plaintext);
    }

    proptest! {
        #[test]
        fn parse_never_panics(s in ".{0,120}") {
            let _ = parse("app", &s);
        }
    }
}
