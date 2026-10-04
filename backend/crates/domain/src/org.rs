use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::ValidationError;

/// URL-safe organisation identifier: lowercase letters, digits and single hyphens, 3-48 chars.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, type = "string")]
#[serde(try_from = "String", into = "String")]
pub struct Slug(String);

/// Slugs that would collide with application routes or be confusing.
const RESERVED: &[&str] = &["admin", "api", "auth", "account", "new", "settings", "system", "www", "app", "org"];

impl Slug {
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let s = raw.trim().to_ascii_lowercase();
        let err = |m: &str| Err(ValidationError::new("slug", m));
        if !(3..=48).contains(&s.len()) {
            return err("must be 3-48 characters");
        }
        if !s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            || s.starts_with('-')
            || s.ends_with('-')
            || s.contains("--")
        {
            return err("may contain lowercase letters, digits and single hyphens");
        }
        if RESERVED.contains(&s.as_str()) {
            return err("is reserved");
        }
        Ok(Self(s))
    }

    /// Derive a candidate slug from a display name (callers append a suffix on collision).
    pub fn suggest(name: &str) -> String {
        let mut out = String::new();
        for c in name.to_ascii_lowercase().chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c);
            } else if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
        // Truncate first, then trim: a cut can land right after a hyphen.
        let truncated: String = out.chars().take(40).collect();
        let mut out = truncated.trim_end_matches('-').to_string();
        while out.len() < 3 {
            out.push('x');
        }
        if RESERVED.contains(&out.as_str()) {
            out.push_str("-org");
        }
        out
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for Slug {
    type Error = ValidationError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        Slug::parse(&s)
    }
}

impl From<Slug> for String {
    fn from(s: Slug) -> String {
        s.0
    }
}

pub fn validate_org_name(raw: &str) -> Result<String, ValidationError> {
    let s = raw.trim();
    if s.is_empty() || s.chars().count() > 80 || s.chars().any(char::is_control) {
        return Err(ValidationError::new("name", "must be 1-80 printable characters"));
    }
    Ok(s.to_string())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export)]
pub struct Organization {
    pub id: Uuid,
    pub slug: Slug,
    pub name: String,
    /// Personal organisations are created automatically for every user and have one member.
    pub personal: bool,
    #[serde(with = "time::serde::rfc3339")]
    #[ts(type = "string")]
    pub created_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn slug_rules() {
        assert_eq!(Slug::parse(" Acme-Corp ").expect("ok").as_str(), "acme-corp");
        for bad in ["ab", "-acme", "acme-", "ac--me", "acme_corp", "admin", "a".repeat(49).as_str()] {
            assert!(Slug::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn suggest_regression_truncation_after_hyphen() {
        // Found by proptest: 40-char cut ending in '-' produced an invalid slug.
        let s = Slug::suggest("a\0aa¡a A¡a a¡0\u{b}00A A¡A¡A\u{b}AAA¡a Aa¡A¡a¡A A");
        assert!(Slug::parse(&s).is_ok(), "{s}");
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(2000))]
        #[test]
        fn suggested_slugs_are_valid(name in ".{0,100}") {
            let s = Slug::suggest(&name);
            prop_assert!(Slug::parse(&s).is_ok(), "{s:?} from {name:?}");
        }
    }
}
