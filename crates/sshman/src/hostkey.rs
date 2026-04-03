//! Host key generation and persistence.
//!
//! On first boot the SSH server generates an Ed25519 keypair and writes it to
//! [`HOST_KEY_PATH`].  Subsequent boots load the existing key so clients can
//! verify the host identity.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
};

use miette::IntoDiagnostic;
use ssh_key::PrivateKey;
use tracing::info;

/// Default location for the host private key.
const HOST_KEY_PATH: &str = "/etc/ssh/host_key";

/// Load an existing host key from disk, or generate a new Ed25519 keypair and
/// persist it.
pub fn load_or_generate() -> miette::Result<PrivateKey> {
    let path = Path::new(HOST_KEY_PATH);

    if path.exists() {
        info!("Loading host key from {HOST_KEY_PATH}");
        let pem = fs::read_to_string(path).into_diagnostic()?;
        let key = russh_keys::decode_secret_key(&pem, None).into_diagnostic()?;
        return Ok(key);
    }

    info!("No host key found — generating new Ed25519 keypair");
    let key = PrivateKey::random(
        &mut ssh_key::rand_core::OsRng,
        ssh_key::Algorithm::Ed25519,
    )
    .into_diagnostic()?;

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).into_diagnostic()?;
    }

    let mut pem_buf = Vec::new();
    russh_keys::encode_pkcs8_pem(&key, &mut pem_buf).into_diagnostic()?;
    fs::write(path, &pem_buf).into_diagnostic()?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).into_diagnostic()?;

    info!("Host key written to {HOST_KEY_PATH}");
    Ok(key)
}
