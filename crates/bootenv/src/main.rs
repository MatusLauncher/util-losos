//! # bootenv
//!
//! A minimal, safe stage-1 initramfs environment written entirely in Rust.
//!
//! # Boot flow
//!
//! 1. Mount virtual filesystems (`devtmpfs`, `proc`, `sysfs`).
//! 2. Parse `/proc/cmdline` for `stage2=` and optional `boot_luks=` parameters.
//! 3. If `boot_luks=` is present, unlock the encrypted boot partition.
//! 4. Provision the stage-2 root filesystem (tarball extraction or block mount).
//! 5. `switch_root` into the real root and exec `/bin/init`.
//!
//! # Kernel parameters
//!
//! | Parameter | Required | Description |
//! |-----------|----------|-------------|
//! | `stage2`  | Yes | Path to the stage-2 archive (`.tar.gz`) or block device (`/dev/sda2`). |
//! | `boot_luks` | No | Path to a LUKS-encrypted boot partition. |
//! | `boot_luks_keyfile` | No | Path to a keyfile for the LUKS boot partition. |

#![cfg(target_os = "linux")]
#![warn(clippy::all, clippy::pedantic)]

mod cmdline;
mod luks;
mod provider;

use std::process::{Command, exit};

use miette::{Context, IntoDiagnostic, Result};
use tracing::info;
use tracing_subscriber::FmtSubscriber;

use cmdline::BootCmdline;
use provider::{REAL_ROOT, Stage2Provider};

fn main() -> Result<()> {
    FmtSubscriber::builder()
        .with_target(false)
        .with_level(true)
        .init();

    if std::process::id() != 1 {
        tracing::warn!(
            "bootenv is not running as PID 1 — some operations may fail"
        );
    }

    info!("bootenv starting — stage 1 initramfs");

    // Step 1: Mount virtual filesystems
    mount_virtual_filesystems()?;

    // Step 2: Parse cmdline
    let bc = BootCmdline::new()?;
    let stage2_value = bc.stage2()?;

    // Step 3: Unlock encrypted boot partition if configured
    let boot_luks_mapper = if bc.has_boot_luks() {
        let device = bc.boot_luks().unwrap();
        let keyfile = bc.boot_luks_keyfile();
        let mapper = luks::unlock_boot_partition(&device, keyfile.as_deref())?;
        Some(mapper)
    } else {
        None
    };

    // Step 4: Build stage-2 provider and provision /real_root
    let provider = Stage2Provider::from_stage2(
        &stage2_value,
        boot_luks_mapper.as_deref(),
    )?;

    smol::block_on(provider.provision())?;

    // Step 5: switch_root into the real root
    info!("Handing over to real init via switch_root");
    exec_switch_root()?;

    // Unreachable — switch_root replaces this process
    unreachable!("switch_root should have replaced this process")
}

/// Mount the essential virtual filesystems.
fn mount_virtual_filesystems() -> Result<()> {
    use rustix::mount::{MountFlags, mount};
    use std::fs::create_dir_all;

    let mounts = [("devtmpfs", "/dev"), ("proc", "/proc"), ("sysfs", "/sys")];

    for (fstype, mountpoint) in mounts {
        create_dir_all(mountpoint).into_diagnostic()?;
        info!("Mounting {fstype} on {mountpoint}");
        mount(
            fstype,
            mountpoint,
            fstype,
            MountFlags::empty(),
            None::<&std::ffi::CStr>,
        )
        .into_diagnostic()
        .wrap_err_with(|| {
            format!("failed to mount {fstype} on {mountpoint}")
        })?;
    }

    Ok(())
}

/// Exec `switch_root` to hand over to the real init.
fn exec_switch_root() -> Result<()> {
    let status = Command::new("switch_root")
        .arg(REAL_ROOT)
        .arg("/bin/init")
        .status()
        .into_diagnostic()
        .wrap_err("failed to exec switch_root")?;

    if !status.success() {
        exit(status.code().unwrap_or(1));
    }

    Ok(())
}
