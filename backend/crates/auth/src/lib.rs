//! Authentication primitives. Nothing here implements cryptography: it composes vetted
//! libraries (openidconnect, jsonwebtoken, RustCrypto AES-GCM/HMAC/SHA-2, OS randomness).
//!
//! - [`oidc`]: standards-based login (authorization code + PKCE + nonce) against any OIDC
//!   provider; ZITADEL is the default deployment. ID tokens are verified by `openidconnect`.
//! - [`tokens`]: opaque session/CSRF/invitation tokens and their at-rest hashes.
//! - [`crypto`]: AES-256-GCM for IdP tokens kept server-side.
//! - [`api_keys`]: `<prefix>_<env>_<id>_<secret>` keys, HMAC-SHA256 hashed with a pepper.
//! - [`service`]: machine-to-machine JWT bearer verification via the IdP's JWKS.
//! - [`idp_admin`]: optional provider-specific administration (passkeys/MFA listing), kept
//!   behind a trait so business logic never depends on a vendor API.

pub mod api_keys;
pub mod crypto;
pub mod http_client;
pub mod idp_admin;
pub mod oidc;
pub mod service;
pub mod tokens;

/// `amr` (Authentication Methods References, RFC 8176) values that indicate multi-factor or
/// phishing-resistant authentication. A passkey (`hwk`/`swk` + user verification) counts as MFA.
pub const MFA_AMR: &[&str] = &["mfa", "otp", "hwk", "swk", "user", "fido", "webauthn", "sms", "mca"];

/// Did this authentication satisfy MFA according to the IdP's `amr` claim?
pub fn amr_is_mfa(amr: &[String]) -> bool {
    amr.iter().any(|m| MFA_AMR.contains(&m.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mfa_detection_from_amr() {
        assert!(!amr_is_mfa(&["pwd".into()]));
        assert!(amr_is_mfa(&["pwd".into(), "otp".into()]));
        assert!(amr_is_mfa(&["hwk".into(), "user".into()]));
        assert!(!amr_is_mfa(&[]));
    }
}
