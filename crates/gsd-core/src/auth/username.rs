//! Validated usernames.

use std::fmt;

use serde::Serialize;

use crate::auth::AuthError;

/// A username: 3–32 characters of `A-Z a-z 0-9 _ . -`. Comparison in the database is
/// case-insensitive, but the original spelling is kept for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct Username(String);

impl Username {
    pub const MIN_LEN: usize = 3;
    pub const MAX_LEN: usize = 32;

    pub fn parse(raw: &str) -> Result<Self, AuthError> {
        let name = raw.trim();
        if !(Self::MIN_LEN..=Self::MAX_LEN).contains(&name.len()) {
            return Err(AuthError::InvalidUsername("must be 3 to 32 characters long"));
        }
        let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-');
        if !name.chars().all(allowed) {
            return Err(AuthError::InvalidUsername(
                "only letters, digits, '_', '.' and '-' are allowed",
            ));
        }
        Ok(Self(name.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Username {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_reasonable_names_and_trims() {
        assert_eq!(Username::parse("  steve_01 ").unwrap().as_str(), "steve_01");
    }

    #[test]
    fn rejects_bad_names() {
        assert!(Username::parse("ab").is_err());
        assert!(Username::parse("has space").is_err());
        assert!(Username::parse(&"x".repeat(33)).is_err());
    }
}
