//! Password policy and Argon2id hashing.
//!
//! Hashing is CPU-heavy (~tens of milliseconds); async callers run it on the blocking pool.

use argon2::{
    Argon2,
    password_hash::{PasswordHasher, PasswordVerifier, phc::PasswordHash},
};

use crate::auth::AuthError;

pub const MIN_LEN: usize = 10;
pub const MAX_LEN: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("failed to hash the password: {0}")]
    Hash(String),
    #[error("a stored password hash is not a valid PHC string: {0}")]
    CorruptHash(String),
}

/// Checks a new password against the policy (length only; passphrases are encouraged).
pub fn validate_new(password: &str) -> Result<(), AuthError> {
    let length = password.chars().count();
    if length < MIN_LEN {
        return Err(AuthError::WeakPassword("must be at least 10 characters long"));
    }
    if length > MAX_LEN {
        return Err(AuthError::WeakPassword("must be at most 256 characters long"));
    }
    Ok(())
}

/// Hashes a password into a PHC string (`$argon2id$v=19$…`) with a fresh random salt.
pub fn hash(password: &str) -> Result<String, PasswordError> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|error| PasswordError::Hash(error.to_string()))
}

/// Returns whether `password` matches the stored PHC hash.
pub fn verify(password: &str, stored_hash: &str) -> Result<bool, PasswordError> {
    let parsed =
        PasswordHash::new(stored_hash).map_err(|error| PasswordError::CorruptHash(error.to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify() {
        let stored = hash("correct horse battery").unwrap();
        assert!(stored.starts_with("$argon2id$"));
        assert!(verify("correct horse battery", &stored).unwrap());
        assert!(!verify("wrong password!", &stored).unwrap());
    }

    #[test]
    fn corrupt_hashes_are_errors_not_mismatches() {
        assert!(verify("anything", "not-a-hash").is_err());
    }

    #[test]
    fn policy_enforces_length() {
        assert!(validate_new("short").is_err());
        assert!(validate_new("long enough pass").is_ok());
    }
}
