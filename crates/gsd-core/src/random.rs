//! Cryptographically secure random values (session tokens, setup tokens, temporary passwords).

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};

/// The operating system's random number generator failed (practically never happens).
#[derive(Debug, thiserror::Error)]
#[error("the system random number generator failed: {0}")]
pub struct RandomError(String);

/// Fills an array of `N` bytes from the OS CSPRNG.
pub fn random_bytes<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut buffer = [0_u8; N];
    getrandom::fill(&mut buffer).map_err(|error| RandomError(error.to_string()))?;
    Ok(buffer)
}

/// `N` random bytes encoded as URL-safe base64 without padding.
pub fn random_token<const N: usize>() -> Result<String, RandomError> {
    Ok(URL_SAFE_NO_PAD.encode(random_bytes::<N>()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_unique_and_url_safe() {
        let a = random_token::<32>().unwrap();
        let b = random_token::<32>().unwrap();
        assert_ne!(a, b);
        assert_eq!(a.len(), 43); // 32 bytes → 43 base64 chars without padding
        assert!(
            a.chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
    }
}
