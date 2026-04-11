//! LUKS2 unlock support for encrypted boot partitions.
//!
//! Provides a single function [`unlock_boot_partition`] that:
//!
//! 1. Probes the given device for a LUKS2 header.
//! 2. Obtains a passphrase (keyfile → console prompt).
//! 3. Unlocks the device as `/dev/mapper/cryptboot`.

use std::{
    fs::File,
    io::{BufRead, BufReader, Write},
    path::Path,
};

use miette::{Context, IntoDiagnostic};
use tracing::info;

#[cfg(feature = "luks")]
use luks::{KeySlotId, LuksHeader, UnlockKey};

/// Unlock an encrypted boot partition and return the mapper device path.
///
/// If the device is not LUKS, the original path is returned unchanged.
///
/// # Parameters
///
/// - `device` — path to the boot partition (e.g. `/dev/sda1`).
/// - `keyfile` — optional keyfile; if absent, prompts on `/dev/console`.
///
/// # Returns
///
/// - `Ok("/dev/mapper/cryptboot")` when LUKS was detected and unlocked.
/// - `Ok(<original_device>)` when the device is plain (not LUKS).
#[cfg(feature = "luks")]
pub fn unlock_boot_partition(
    device: &str,
    keyfile: Option<&str>,
) -> miette::Result<String> {
    if !is_luks(device) {
        info!("Boot device {device} is not encrypted — using as-is");
        return Ok(device.to_string());
    }

    info!("LUKS2 header detected on boot device {device}");
    let passphrase = obtain_passphrase(keyfile)?;
    unlock_with_passphrase(device, "cryptboot", &passphrase)?;
    info!("Boot partition unlocked as /dev/mapper/cryptboot");
    Ok("/dev/mapper/cryptboot".to_string())
}

/// Dummy fallback when the `luks` feature is disabled — always returns the
/// original device path.
#[cfg(not(feature = "luks"))]
pub fn unlock_boot_partition(
    device: &str,
    _keyfile: Option<&str>,
) -> miette::Result<String> {
    warn!("LUKS feature disabled — boot partition cannot be unlocked");
    Ok(device.to_string())
}

// ── LUKS internals (only compiled when `luks` feature is enabled) ─────────────

#[cfg(feature = "luks")]
fn is_luks(device: &str) -> bool {
    File::open(device)
        .ok()
        .and_then(|f| LuksHeader::open(f).ok())
        .is_some()
}

#[cfg(feature = "luks")]
fn unlock_with_passphrase(
    device: &str,
    map_name: &str,
    passphrase: &str,
) -> miette::Result<()> {
    info!("Unlocking LUKS2 device {device} as /dev/mapper/{map_name}");
    let dev_path = Path::new(device);
    let file = File::open(dev_path).into_diagnostic()?;
    let mut luks_device = LuksHeader::open(file).into_diagnostic()?;
    let keyslot_id = KeySlotId::from(0u32);
    let unlock_key = UnlockKey::from_passphrase(passphrase.to_string());
    luks_device
        .unlock(&keyslot_id, &unlock_key)
        .into_diagnostic()
        .wrap_err("failed to unlock LUKS device")?;
    luks_device
        .map_with_dmsetup(map_name, dev_path)
        .into_diagnostic()
        .wrap_err("failed to create dm-crypt mapping")?;
    Ok(())
}

#[cfg(feature = "luks")]
fn obtain_passphrase(keyfile: Option<&str>) -> miette::Result<String> {
    if let Some(path) = keyfile {
        info!("Reading LUKS keyfile from {path}");
        return std::fs::read_to_string(path)
            .into_diagnostic()
            .map(|s| s.trim().to_string());
    }
    read_passphrase_from_console()
}

#[cfg(feature = "luks")]
fn read_passphrase_from_console() -> miette::Result<String> {
    use rustix::termios::{self, OptionalActions, Termios};

    let mut console = File::options()
        .read(true)
        .write(true)
        .open("/dev/console")
        .into_diagnostic()?;

    let orig_termios: Termios =
        termios::tcgetattr(&console).into_diagnostic()?;
    let mut noecho = orig_termios.clone();
    noecho.local_modes &= !termios::LocalModes::ECHO;
    termios::tcsetattr(&console, OptionalActions::Now, &noecho)
        .into_diagnostic()?;

    console
        .write_all(b"Enter LUKS passphrase: ")
        .into_diagnostic()?;
    console.flush().into_diagnostic()?;

    let mut passphrase = String::new();
    BufReader::new(&console)
        .read_line(&mut passphrase)
        .into_diagnostic()?;

    termios::tcsetattr(&console, OptionalActions::Now, &orig_termios)
        .into_diagnostic()?;
    console.write_all(b"\n").into_diagnostic()?;

    Ok(passphrase.trim().to_string())
}
