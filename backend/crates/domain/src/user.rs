//! Application-side identity. Credentials, MFA, passkeys and linked identities are owned by
//! the identity provider; the application stores a reference (`identity_provider`,
//! `external_subject`) plus application-owned profile data. See
//! docs/authentication/data-ownership.md for the field-by-field source of truth.

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ValidationError;

/// Organisation-level role. Ordered by privilege: owner ⊇ admin ⊇ manager ⊇ member ⊇ viewer.
/// The permissions each role grants are defined in `app-authz`, not here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum OrgRole {
    Viewer,
    Member,
    Manager,
    Admin,
    Owner,
}

impl OrgRole {
    pub const ALL: [OrgRole; 5] = [OrgRole::Viewer, OrgRole::Member, OrgRole::Manager, OrgRole::Admin, OrgRole::Owner];

    pub fn as_str(self) -> &'static str {
        match self {
            OrgRole::Viewer => "viewer",
            OrgRole::Member => "member",
            OrgRole::Manager => "manager",
            OrgRole::Admin => "admin",
            OrgRole::Owner => "owner",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

/// System (platform operator) trust level. **Independent** of organisation roles: an
/// organisation owner or admin is `SystemRole::None` unless separately granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SystemRole {
    #[default]
    None,
    /// Read-only access to system administration (support, auditors).
    SystemAuditor,
    /// Full system administration.
    SystemAdmin,
}

impl SystemRole {
    pub fn as_str(self) -> &'static str {
        match self {
            SystemRole::None => "none",
            SystemRole::SystemAuditor => "system_auditor",
            SystemRole::SystemAdmin => "system_admin",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "none" => Some(SystemRole::None),
            "system_auditor" => Some(SystemRole::SystemAuditor),
            "system_admin" => Some(SystemRole::SystemAdmin),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Active,
    Suspended,
    Deleted,
}

impl UserStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            UserStatus::Active => "active",
            UserStatus::Suspended => "suspended",
            UserStatus::Deleted => "deleted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "active" => Some(UserStatus::Active),
            "suspended" => Some(UserStatus::Suspended),
            "deleted" => Some(UserStatus::Deleted),
            _ => None,
        }
    }
}

/// A syntactically plausible, normalised (trimmed, lower-cased) email address.
/// The identity provider verifies ownership; this type only rejects garbage input.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, type = "string")]
#[serde(try_from = "String", into = "String")]
pub struct Email(String);

impl Email {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let s = raw.trim().to_ascii_lowercase();
        let err = |m: &str| Err(ValidationError::new("email", m));
        if s.len() > 254 {
            return err("too long");
        }
        let Some((local, domain)) = s.split_once('@') else { return err("must contain @") };
        if local.is_empty() || domain.len() < 3 || !domain.contains('.') || domain.contains('@') {
            return err("is not a valid address");
        }
        if s.chars().any(|c| c.is_whitespace() || c.is_control()) {
            return err("must not contain whitespace");
        }
        Ok(Self(s))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn domain(&self) -> &str {
        self.0.rsplit_once('@').map_or("", |(_, d)| d)
    }
}

impl TryFrom<String> for Email {
    type Error = ValidationError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Email::parse(&s)
    }
}

impl From<Email> for String {
    fn from(e: Email) -> String {
        e.0
    }
}

/// Validated display name (application-owned profile data).
pub fn validate_display_name(raw: &str) -> Result<String, ValidationError> {
    let s = raw.trim();
    let n = s.chars().count();
    if n == 0 || n > 80 {
        return Err(ValidationError::new("display_name", "must be 1-80 characters"));
    }
    if s.chars().any(char::is_control) {
        return Err(ValidationError::new("display_name", "must not contain control characters"));
    }
    Ok(s.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export)]
pub struct User {
    pub id: Uuid,
    /// Cached from the identity provider on each login; the IdP is the source of truth.
    pub email: Email,
    pub email_verified: bool,
    pub display_name: String,
    pub status: UserStatus,
    pub system_role: SystemRole,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn email_normalises_and_rejects_garbage() {
        let e = Email::parse("  Ada@Example.COM ").expect("valid");
        assert_eq!(e.as_str(), "ada@example.com");
        assert_eq!(e.domain(), "example.com");
        for bad in ["", "no-at", "@x.io", "a@b", "a@@b.io", "a b@c.io"] {
            assert!(Email::parse(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn org_role_order_and_round_trip() {
        assert!(OrgRole::Owner > OrgRole::Admin && OrgRole::Admin > OrgRole::Manager);
        assert!(OrgRole::Member > OrgRole::Viewer);
        for r in OrgRole::ALL {
            assert_eq!(OrgRole::parse(r.as_str()), Some(r));
        }
        assert_eq!(OrgRole::parse("system_admin"), None, "system roles are not org roles");
    }

    #[test]
    fn system_role_is_separate() {
        for r in [SystemRole::None, SystemRole::SystemAuditor, SystemRole::SystemAdmin] {
            assert_eq!(SystemRole::parse(r.as_str()), Some(r));
        }
        assert_eq!(SystemRole::parse("owner"), None);
    }

    #[test]
    fn display_name_validation() {
        assert_eq!(validate_display_name("  Ada  ").expect("ok"), "Ada");
        assert!(validate_display_name("").is_err());
        assert!(validate_display_name("a\u{0007}b").is_err());
    }

    proptest! {
        #[test]
        fn email_parse_never_panics_and_is_idempotent(s in ".{0,300}") {
            if let Ok(e) = Email::parse(&s) {
                prop_assert_eq!(Email::parse(e.as_str()).expect("re-parse"), e.clone());
            }
        }
    }
}
