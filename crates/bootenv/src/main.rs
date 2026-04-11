//! # bootenv
//!
//! A minimal, safe stage-1 initramfs environment written entirely in Rust.
//!
//! # Boot flow
//!
//! 1. Mount virtual filesystems (`devtmpfs`, `proc`, `sysfs`, `tmpfs`) on the
//!    initramfs root so `/proc/cmdline` is readable.
//! 2. Parse `/proc/cmdline` for `data_drive=`.
//! 3. Activate block layers and mount the data volume to `/data`.
//! 4. Exec `/bin/init` (actman) directly — the initramfs is the permanent root.
//!
//! # Kernel parameters
//!
//! | Parameter | Required | Description |
//! |-----------|----------|-------------|
//! | `data_drive` | No | Semicolon-separated block/NFS URIs for the persistent data volume. When absent, the OS runs entirely in RAM. |

#![cfg(target_os = "linux")]
#![warn(clippy::all, clippy::pedantic)]

mod cmdline;

use std::{
    ffi::CStr, fs::create_dir_all, os::unix::process::CommandExt,
    process::Command,
};

use miette::{Context, IntoDiagnostic, Result};
use tracing::info;
use tracing_subscriber::FmtSubscriber;

use actman::dataspec::DataSpec;
use actman::nfs::mount_nfs;
use cmdline::BootCmdline;

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

    // Step 1: Mount virtual filesystems on the initramfs root so /proc is
    // available for reading the kernel command line.
    mount_virtual_filesystems()?;

    // Step 2: Parse cmdline
    let bc = BootCmdline::new()?;

    // Step 3: Activate block layers and mount the data drive to /data.
    mount_data_drive(&bc)?;

    // Step 4: Exec actman as the permanent PID 1 — the initramfs is the root.
    info!("Handing over to actman (/bin/init)");
    let err = Command::new("/bin/init").exec();

    // exec() only returns on failure
    Err(miette::miette!("failed to exec /bin/init: {err}"))
}

/// Mount the essential virtual filesystems on the initramfs root.
fn mount_virtual_filesystems() -> Result<()> {
    use rustix::mount::{MountFlags, mount};

    let mounts = [
        ("devtmpfs", "/dev"),
        ("proc", "/proc"),
        ("sysfs", "/sys"),
        ("tmpfs", "/tmp"),
    ];

    for (fstype, mountpoint) in mounts {
        create_dir_all(mountpoint).into_diagnostic()?;
        info!("Mounting {fstype} on {mountpoint}");
        mount(
            fstype,
            mountpoint,
            fstype,
            MountFlags::empty(),
            None::<&CStr>,
        )
        .into_diagnostic()
        .wrap_err_with(|| {
            format!("failed to mount {fstype} on {mountpoint}")
        })?;
    }

    Ok(())
}

/// Parse `data_drive=`, activate block layers, and mount the data volume to
/// `/data`.  Additional NFS entries with explicit `mountpoint=` are mounted
/// to `/<mountpoint>`.
///
/// Returns `Ok(())` immediately when `data_drive=` is absent (RAM-only mode).
fn mount_data_drive(bc: &BootCmdline) -> Result<()> {
    use rustix::mount::{MountFlags, mount};

    let Some(data_drive_str) = bc.data_drive() else {
        tracing::warn!(
            "No data_drive= kernel parameter — OS running entirely in RAM"
        );
        return Ok(());
    };

    let data_specs: Vec<DataSpec> = DataSpec::parse_list(data_drive_str)?;

    let block_drive = data_specs.iter().find_map(|e| e.as_block());
    let nfs_drive = data_specs
        .iter()
        .find_map(|e| e.as_nfs().filter(|n| n.mountpoint.is_none()));

    // ── Block-layer activation ────────────────────────────────────────────
    // LUKS bulk-unlock → LVM activation → MD RAID assembly.
    if let Some(spec) = block_drive {
        if !spec.luks_devices.is_empty() {
            actman::luks::unlock_all(
                &spec.luks_devices,
                spec.keyfile.as_deref(),
            )?;
        }
        if spec.lvm {
            actman::lvm::activate()?;
        }
        if spec.raid {
            actman::raid::assemble()?;
        }
    }

    // ── Data drive mount ─────────────────────────────────────────────────
    // Mount directly to /data — the initramfs is the permanent root.
    create_dir_all("/data").into_diagnostic()?;

    if let Some(spec) = block_drive {
        let mount_target = if spec.luks {
            actman::luks::probe_and_unlock(
                &spec.device,
                spec.keyfile.as_deref(),
            )?
        } else {
            spec.device.clone()
        };
        info!("Mounting data drive ({mount_target}) to /data");
        mount(
            mount_target.as_str(),
            "/data",
            "",
            MountFlags::empty(),
            None::<&CStr>,
        )
        .into_diagnostic()?;
    } else if let Some(nfs) = nfs_drive {
        mount_nfs(&nfs.server_export, "/data", &nfs.opts)?;
    }

    // ── Additional NFS mounts ─────────────────────────────────────────────
    // Entries with explicit mountpoint= are independent extra mounts.
    for nfs in data_specs.iter().filter_map(|e| e.as_nfs()) {
        if let Some(ref mp) = nfs.mountpoint {
            create_dir_all(mp).into_diagnostic()?;
            mount_nfs(&nfs.server_export, mp, &nfs.opts)?;
        }
    }

    Ok(())
}
