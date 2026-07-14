use bcrypt::{DEFAULT_COST, hash, verify};

/// Minimum password length enforced at registration / password-change time.
/// Closes 03-user F-05 / 13-misc H-1 / 06-llm-provider weak-password gap.
pub const MIN_PASSWORD_LENGTH: usize = 8;

/// Maximum password length. bcrypt silently truncates after byte 72
/// (the password-input length is the LIMIT of the bcrypt algorithm), so
/// rejecting >72 bytes prevents the silent-truncation hazard the audit
/// flagged in 13-misc H-1 (setup admin).
pub const MAX_PASSWORD_LENGTH: usize = 72;

/// Validate password strength at registration / change time.
///
/// Returns Err with a human-readable, NON-leaky reason on failure.
/// Length-only enforcement is the floor — complexity rules are out of
/// scope here per NIST SP 800-63B guidance (length is the dominant
/// strength dimension; complexity rules push users toward predictable
/// patterns).
pub fn validate_password_strength(password: &str) -> Result<(), &'static str> {
    if password.len() < MIN_PASSWORD_LENGTH {
        return Err("Password must be at least 8 characters long");
    }
    if password.len() > MAX_PASSWORD_LENGTH {
        return Err(
            "Password must be at most 72 bytes (bcrypt truncates beyond that)",
        );
    }
    if password.bytes().any(|b| b == 0) {
        return Err("Password must not contain NUL bytes");
    }
    Ok(())
}

/// Hash a password using bcrypt
/// bcrypt automatically generates and includes salt in the hash
pub fn hash_password(password: &str) -> Result<String, bcrypt::BcryptError> {
    hash(password, DEFAULT_COST)
}

/// Verify a password against a stored hash
pub fn verify_password(password: &str, hash: &str) -> Result<bool, bcrypt::BcryptError> {
    verify(password, hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_password_hashing() {
        let password = "test_password_123";
        let hashed = hash_password(password).unwrap();

        assert!(verify_password(password, &hashed).unwrap());
        assert!(!verify_password("wrong_password", &hashed).unwrap());
    }

    #[test]
    fn rejects_empty_password() {
        assert!(validate_password_strength("").is_err());
    }

    #[test]
    fn rejects_short_password() {
        assert!(validate_password_strength("short").is_err());
    }

    #[test]
    fn accepts_8_char_password() {
        assert!(validate_password_strength("abcdefgh").is_ok());
    }

    #[test]
    fn rejects_password_over_72_bytes() {
        // 73 bytes — exceeds bcrypt's silent-truncation limit.
        let too_long = "a".repeat(73);
        assert!(validate_password_strength(&too_long).is_err());
    }

    #[test]
    fn accepts_password_at_exactly_72_bytes() {
        let max = "a".repeat(72);
        assert!(validate_password_strength(&max).is_ok());
    }

    #[test]
    fn rejects_password_with_nul_byte() {
        let nul = "abcdefgh\0extra";
        assert!(validate_password_strength(nul).is_err());
    }

    #[test]
    fn test_different_hashes() {
        let password = "same_password";
        let hash1 = hash_password(password).unwrap();
        let hash2 = hash_password(password).unwrap();

        // bcrypt generates different salts each time
        assert_ne!(hash1, hash2);

        // But both verify correctly
        assert!(verify_password(password, &hash1).unwrap());
        assert!(verify_password(password, &hash2).unwrap());
    }
}
