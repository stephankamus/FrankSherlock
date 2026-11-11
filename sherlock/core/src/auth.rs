//! Password and account-field rules shared by the desktop writer and the web verifier.
//!
//! The desktop stores only an Argon2id PHC string. The web server never writes a user;
//! it verifies a submitted password against the string that travelled inside the
//! published snapshot. Keeping both operations in this module is what guarantees the two
//! sides agree on the format and on the input limits.

use argon2::password_hash::rand_core::OsRng;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

use crate::error::{AppError, AppResult};

pub const MIN_PASSWORD_LEN: usize = 8;
pub const MAX_PASSWORD_LEN: usize = 256;
pub const MIN_LOGIN_LEN: usize = 3;
pub const MAX_LOGIN_LEN: usize = 64;
pub const MAX_NAME_LEN: usize = 120;

/// Hash a password into a self-describing Argon2id PHC string.
pub fn hash_password(password: &str) -> AppResult<String> {
    validate_password(password).map_err(AppError::Config)?;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| AppError::Config(format!("could not hash the password: {error}")))
}

/// Verify a password against a stored PHC string.
///
/// A missing or corrupt hash fails closed instead of panicking: an old or truncated
/// snapshot must not turn into an authentication bypass.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else {
        return false;
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

pub fn validate_password(password: &str) -> Result<(), String> {
    let length = password.chars().count();
    if length < MIN_PASSWORD_LEN {
        return Err(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        ));
    }
    if length > MAX_PASSWORD_LEN {
        return Err(format!(
            "password must be at most {MAX_PASSWORD_LEN} characters"
        ));
    }
    Ok(())
}

/// Normalize and validate the display name.
pub fn validate_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("name is required".to_string());
    }
    if trimmed.chars().count() > MAX_NAME_LEN {
        return Err(format!("name must be at most {MAX_NAME_LEN} characters"));
    }
    Ok(trimmed.to_string())
}

/// Normalize and validate the login.
///
/// Whitespace and control characters are refused rather than silently rewritten: the
/// login is what the user types into the web form, and a value that cannot be typed back
/// exactly would be a support problem.
pub fn validate_login(login: &str) -> Result<String, String> {
    let trimmed = login.trim();
    let length = trimmed.chars().count();
    if length < MIN_LOGIN_LEN {
        return Err(format!("login must be at least {MIN_LOGIN_LEN} characters"));
    }
    if length > MAX_LOGIN_LEN {
        return Err(format!("login must be at most {MAX_LOGIN_LEN} characters"));
    }
    if trimmed.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("login must not contain spaces or control characters".to_string());
    }
    Ok(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_are_salted_and_verifiable() {
        let first = hash_password("correct horse battery staple").expect("hash");
        let second = hash_password("correct horse battery staple").expect("hash");
        assert!(first.starts_with("$argon2id$"));
        assert_ne!(first, second, "each hash must use a fresh salt");
        assert!(verify_password("correct horse battery staple", &first));
        assert!(!verify_password("wrong password", &first));
    }

    #[test]
    fn malformed_hashes_fail_closed() {
        assert!(!verify_password("anything", ""));
        assert!(!verify_password("anything", "not-a-phc-string"));
    }

    #[test]
    fn password_policy_is_shared() {
        assert!(validate_password("1234567").is_err());
        assert!(validate_password("12345678").is_ok());
        assert!(validate_password(&"x".repeat(MAX_PASSWORD_LEN + 1)).is_err());
    }

    #[test]
    fn account_fields_are_trimmed_and_checked() {
        assert_eq!(validate_name("  Ana  ").expect("name"), "Ana");
        assert!(validate_name("   ").is_err());
        assert_eq!(validate_login("  ana.silva ").expect("login"), "ana.silva");
        assert!(validate_login("ab").is_err());
        assert!(validate_login("ana silva").is_err());
    }
}
