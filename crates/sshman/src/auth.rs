//! Authentication helpers that verify credentials against the userman daemon.

use base64::{Engine, prelude::BASE64_STANDARD};
use miette::{IntoDiagnostic, miette};
use ssh_key::PublicKey;
use totp_rs::{Algorithm, TOTP};
use userman::daemon::UserSchema;

/// Verify a plaintext password against the base64-encoded password stored in
/// the user schema.
pub fn verify_password(user: &UserSchema, password: &str) -> bool {
    let stored = BASE64_STANDARD
        .decode(user.pass())
        .ok()
        .and_then(|b| String::from_utf8(b).ok());
    stored.as_deref() == Some(password)
}

/// Return `Err` if the account is locked out.
pub fn check_account(user: &UserSchema) -> Result<(), &'static str> {
    if user.locked_out() {
        return Err("Account is locked out");
    }
    Ok(())
}

/// Validate a TOTP code for the given user.
pub fn verify_totp(user: &UserSchema, code: &str) -> miette::Result<bool> {
    let secret = user
        .totp_secret()
        .ok_or_else(|| miette!("No TOTP secret configured for user"))?;
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret.to_vec(),
        Some("userman".to_string()),
        String::new(),
    )
    .into_diagnostic()?;
    totp.check_current(code).into_diagnostic()
}

/// Check if `input` matches the user's stored second password.
pub fn verify_second_password(user: &UserSchema, input: &str) -> bool {
    user.second_pass().is_some_and(|stored| stored == input)
}

/// Check if `offered` matches any of the user's registered SSH public keys.
pub fn match_public_key(user: &UserSchema, offered: &PublicKey) -> bool {
    for key_str in user.ssh_public_keys() {
        // OpenSSH authorized_keys format: "<type> <base64> [comment]"
        let parts: Vec<&str> = key_str.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }
        if let Ok(parsed) = russh_keys::parse_public_key_base64(parts[1])
            && parsed == *offered
        {
            return true;
        }
    }
    false
}
