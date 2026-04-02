//! LUKS2 home-directory encryption helpers.
//!
//! Wraps the [`luks`] crate to open an existing LUKS2 container and expose
//! it as a device-mapper device so the user's home directory can be mounted.

use std::{fs::File, path::Path};

use luks::{KeySlotId, LuksHeader, UnlockKey};
use miette::IntoDiagnostic;

/// Open an existing LUKS2 container at `device`, unlock keyslot 0 with
/// `passphrase`, and map it as `/dev/mapper/<map_name>` via dm-crypt.
pub fn unlock_home(device: &Path, passphrase: &str, map_name: &str) -> miette::Result<()> {
    let file = File::open(device).into_diagnostic()?;
    let mut luks_device = LuksHeader::open(file).into_diagnostic()?;
    let keyslot_id = KeySlotId::from(0u32);
    let unlock_key = UnlockKey::from_passphrase(passphrase.to_string());
    luks_device
        .unlock(&keyslot_id, &unlock_key)
        .into_diagnostic()?;
    luks_device
        .map_with_dmsetup(map_name, device)
        .into_diagnostic()
}
