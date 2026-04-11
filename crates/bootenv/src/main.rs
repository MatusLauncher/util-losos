//! # bootenv
//!
//! A minimal, safe stage-1 initramfs environment written entirely in Rust.
//!
//! # Boot flow
//!
//! 1. Mount virtual filesystems (`devtmpfs`, `proc`, `sysfs`, `tmpfs`) on the
//!    initramfs root so `/proc/cmdline` is readable.
//! 2. Parse `/proc/cmdline` for `stage2=`, `boot_luks=`, and `data_drive=`.
//! 3. Unlock the encrypted boot partition if `boot_luks=` is present.
//! 4. Provision the stage-2 root filesystem to `/real_root`.
//! 5. Mount virtual filesystems on `/real_root` so actman inherits them after
//!    `switch_root`.
//! 6. Activate block layers and mount the data drive to `/real_root/data`
//!    (and any additional NFS entries).
//! 7. `switch_root` into the real root and exec `/bin/init`.
//!
//! # Kernel parameters
//!
//! | Parameter | Required | Description |
//! |-----------|----------|-------------|
//! | `stage2`  | Yes | Path to the stage-2 archive (`.tar.gz`) or block device (`/dev/sda2`). |
//! | `boot_luks` | No | Path to a LUKS-encrypted boot partition. |
//! | `boot_luks_keyfile` | No | Path to a keyfile for the LUKS boot partition. |
//! | `data_drive` | No | Semicolon-separated block/NFS URIs for the persistent data volume. |

#![cfg(target_os = "linux")]
#![warn(clippy::all, clippy::pedantic)]

mod cmdline;
mod luks;
mod provider;

use std::{
    ffi::CStr,
    fs::create_dir_all,
    process::{Command, exit},
};

use miette::{Context, IntoDiagnostic, Result};
use tracing::info;
use tracing_subscriber::FmtSubscriber;

use actman::dataspec::DataSpec;
use actman::nfs::mount_nfs;
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

    // Step 1: Mount virtual filesystems on the initramfs root so /proc is
    // available for reading the kernel command line.
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

    // Step 5: Mount virtual filesystems on the real root so actman inherits
    // them after switch_root without needing to remount.
    mount_real_root_virtual_fs()?;

    // Step 6: Activate block layers and mount the data drive to /real_root/data
    mount_data_drive(&bc)?;

    // Step 7: switch_root into the real root
    info!("Handing over to real init via switch_root");
    exec_switch_root()?;

    // Unreachable — switch_root replaces this process
    unreachable!("switch_root should have replaced this process")
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

/// Mount virtual filesystems directly on the real-root paths so they are
/// accessible immediately after `switch_root`, without actman needing to
/// remount them.
fn mount_real_root_virtual_fs() -> Result<()> {
    use rustix::mount::{MountFlags, mount};

    let mounts = [
        ("devtmpfs", "/real_root/dev"),
        ("proc", "/real_root/proc"),
        ("sysfs", "/real_root/sys"),
        ("tmpfs", "/real_root/tmp"),
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
/// `/real_root/data`.  Additional NFS entries with explicit `mountpoint=` are
/// mounted to `/real_root/<mountpoint>`.
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
    // Mount to /real_root/data so the path survives switch_root as /data.
    create_dir_all(format!("{REAL_ROOT}/data")).into_diagnostic()?;

    if let Some(spec) = block_drive {
        let mount_target = if spec.luks {
            actman::luks::probe_and_unlock(
                &spec.device,
                spec.keyfile.as_deref(),
            )?
        } else {
            spec.device.clone()
        };
        info!("Mounting data drive ({mount_target}) to {REAL_ROOT}/data");
        mount(
            mount_target.as_str(),
            format!("{REAL_ROOT}/data").as_str(),
            "",
            MountFlags::empty(),
            None::<&CStr>,
        )
        .into_diagnostic()?;
    } else if let Some(nfs) = nfs_drive {
        mount_nfs(&nfs.server_export, &format!("{REAL_ROOT}/data"), &nfs.opts)?;
    }

    // ── Additional NFS mounts ─────────────────────────────────────────────
    // Entries with explicit mountpoint= are independent extra mounts.
    for nfs in data_specs.iter().filter_map(|e| e.as_nfs()) {
        if let Some(ref mp) = nfs.mountpoint {
            let real_mp = format!("{REAL_ROOT}{mp}");
            create_dir_all(&real_mp).into_diagnostic()?;
            mount_nfs(&nfs.server_export, &real_mp, &nfs.opts)?;
        }
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
