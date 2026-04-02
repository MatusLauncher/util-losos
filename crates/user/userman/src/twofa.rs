//! Two-factor authentication helpers.
//!
//! Supports three second-factor methods:
//!
//! * **TOTP** — RFC 6238 time-based one-time passwords via [`totp_rs`].
//!   Use [`generate_totp_secret`] → [`totp_setup_uri`] for enrolment and
//!   [`validate_totp`] for verification.
//!
//! * **FIDO2 / Passkey** — CTAP HID hardware authenticators via
//!   [`ctap_hid_fido2`]. Use [`register_fido2`] for enrolment and
//!   [`verify_fido2`] for verification.
//!
//! * **Password** — a plain secondary password stored in [`UserSchema`];
//!   comparison is handled directly in `main`.
//!
//! [`UserSchema`]: crate::daemon::UserSchema

use ctap_hid_fido2::{
    Cfg, FidoKeyHidFactory,
    fidokey::{GetAssertionArgsBuilder, MakeCredentialArgsBuilder},
    public_key::{PublicKey, PublicKeyType},
    verifier,
};
use miette::{IntoDiagnostic, miette};
use rand::RngCore;
use totp_rs::{Algorithm, TOTP};

/// Relying-party ID used for all FIDO2 operations.
const RPID: &str = "losOS";

/// Generate 20 random bytes to use as a TOTP secret.
pub fn generate_totp_secret() -> Vec<u8> {
    let mut rng = rand::rng();
    let mut secret = vec![0u8; 20];
    rng.fill_bytes(&mut secret);
    secret
}

/// Return the `otpauth://` provisioning URI for `username`.
pub fn totp_setup_uri(secret: &[u8], username: &str) -> miette::Result<String> {
    let totp = TOTP::new(
        Algorithm::SHA1,
        6,
        1,
        30,
        secret.to_vec(),
        Some("userman".to_string()),
        username.to_string(),
    )
    .into_diagnostic()?;
    Ok(totp.get_url())
}

/// Return `true` if `code` is a valid TOTP token for `secret` at the current time.
pub fn validate_totp(secret: &[u8], code: &str) -> miette::Result<bool> {
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

/// Register a FIDO2 credential on the connected authenticator.
///
/// Returns `(credential_id, public_key_der, public_key_type)`.
/// `public_key_type`: 0 = Unknown, 1 = Ecdsa256, 2 = Ed25519.
pub fn register_fido2(pin: Option<&str>) -> miette::Result<(Vec<u8>, Vec<u8>, u8)> {
    let device = FidoKeyHidFactory::create(&Cfg::init()).map_err(|e| miette!("{:#}", e))?;
    let challenge = verifier::create_challenge();
    let builder = MakeCredentialArgsBuilder::new(RPID, &challenge);
    let builder = match pin {
        Some(p) => builder.pin(p),
        None => builder.without_pin_and_uv(),
    };
    let attestation = device
        .make_credential_with_args(&builder.build())
        .map_err(|e| miette!("{:#}", e))?;
    let result = verifier::verify_attestation(RPID, &challenge, &attestation);
    if !result.is_success {
        return Err(miette!("FIDO2 attestation verification failed"));
    }
    let key_type_u8 = key_type_to_u8(&result.credential_public_key.key_type);
    Ok((
        result.credential_id,
        result.credential_public_key.der,
        key_type_u8,
    ))
}

/// Verify a FIDO2 assertion against a stored credential.
///
/// Returns `true` when the authenticator produces a valid signature.
pub fn verify_fido2(
    credential_id: &[u8],
    public_key_der: &[u8],
    public_key_type: u8,
    pin: Option<&str>,
) -> miette::Result<bool> {
    let device = FidoKeyHidFactory::create(&Cfg::init()).map_err(|e| miette!("{:#}", e))?;
    let challenge = verifier::create_challenge();
    let builder = GetAssertionArgsBuilder::new(RPID, &challenge).credential_id(credential_id);
    let builder = match pin {
        Some(p) => builder.pin(p),
        None => builder.without_pin_and_uv(),
    };
    let assertions = device
        .get_assertion_with_args(&builder.build())
        .map_err(|e| miette!("{:#}", e))?;
    let assertion = assertions
        .first()
        .ok_or_else(|| miette!("No FIDO2 assertion returned"))?;
    let public_key = PublicKey::with_der(public_key_der, key_type_from_u8(public_key_type));
    Ok(verifier::verify_assertion(
        RPID,
        &public_key,
        &challenge,
        assertion,
    ))
}

/// Convert a [`PublicKeyType`] to the compact `u8` representation stored in [`UserSchema`].
fn key_type_to_u8(kt: &PublicKeyType) -> u8 {
    match kt {
        PublicKeyType::Ecdsa256 => 1,
        PublicKeyType::Ed25519 => 2,
        PublicKeyType::Unknown => 0,
    }
}

/// Convert the stored `u8` key-type tag back to a [`PublicKeyType`].
/// Any value other than `1` or `2` maps to [`PublicKeyType::Unknown`].
fn key_type_from_u8(v: u8) -> PublicKeyType {
    match v {
        1 => PublicKeyType::Ecdsa256,
        2 => PublicKeyType::Ed25519,
        _ => PublicKeyType::Unknown,
    }
}
