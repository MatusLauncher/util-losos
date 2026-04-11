//! Stage-2 provider — mounts or extracts the real root filesystem.
//!
//! Three providers are supported:
//!
//! | Provider | `stage2` value | Behaviour |
//! |----------|---------------|-----------|
//! | `Tarball` | path to `.tar.gz` | Extracts to `/real_root` |
//! | `BlockDevice` | path to block device (e.g. `/dev/sda2`) | Mounts ext4 to `/real_root` |
//! | `LuksDevice` | path returned from [`crate::luks::unlock_boot_partition`] | Mounts the unlocked mapper device to `/real_root` |

use std::path::Path;

use miette::{Context, IntoDiagnostic, miette};
use smol::fs;
use tracing::info;

/// The real root mountpoint.
pub const REAL_ROOT: &str = "/real_root";

/// A provider for the stage-2 root filesystem.
#[derive(Debug, Clone)]
pub enum Stage2Provider {
    /// A compressed tarball (`.tar.gz`) that is extracted into RAM.
    Tarball {
        /// Path to the tarball on the boot partition.
        path: String,
    },
    /// A plain (unencrypted) block device to mount.
    BlockDevice {
        /// Device path (e.g. `/dev/sda2`).
        device: String,
    },
    /// An already-unlocked LUKS mapper device.
    LuksDevice {
        /// Mapper path (e.g. `/dev/mapper/cryptboot`).
        mapper: String,
        /// Path to the stage-2 tarball *inside* the unlocked device, or
        /// `None` when the mapper device itself contains the root FS directly.
        stage2_path: Option<String>,
    },
}

impl Stage2Provider {
    /// Construct a provider from the `stage2` cmdline value and an optional
    /// unlocked boot partition path.
    ///
    /// # Resolution logic
    ///
    /// 1. If `boot_luks_mapper` is `Some`, the stage2 path is interpreted as a
    ///    path *inside* the unlocked boot partition.
    /// 2. If the value starts with `/dev/` and is not a LUKS mapper, it is
    ///    treated as a plain block device.
    /// 3. Otherwise it is treated as a tarball path.
    pub fn from_stage2(
        value: &str,
        boot_luks_mapper: Option<&str>,
    ) -> miette::Result<Self> {
        if let Some(mapper) = boot_luks_mapper {
            // Stage2 path lives on the unlocked LUKS boot partition.
            Ok(Self::LuksDevice {
                mapper: mapper.to_string(),
                stage2_path: Some(value.to_string()),
            })
        } else if value.starts_with("/dev/") {
            if value.contains("/mapper/") {
                // Already a dm-crypt mapper device — treat as direct root.
                Ok(Self::LuksDevice {
                    mapper: value.to_string(),
                    stage2_path: None,
                })
            } else {
                Ok(Self::BlockDevice {
                    device: value.to_string(),
                })
            }
        } else {
            Ok(Self::Tarball {
                path: value.to_string(),
            })
        }
    }

    /// Mounts or extracts the stage-2 filesystem to [`REAL_ROOT`].
    pub async fn provision(&self) -> miette::Result<()> {
        // Ensure /real_root exists
        fs::create_dir_all(REAL_ROOT)
            .await
            .into_diagnostic()
            .wrap_err("failed to create /real_root")?;

        match self {
            Self::Tarball { path } => self.extract_tarball(path).await,
            Self::BlockDevice { device } => self.mount_block(device).await,
            Self::LuksDevice {
                mapper,
                stage2_path: Some(path),
            } => {
                // Mount the unlocked boot partition, extract the tarball.
                self.mount_and_extract_from_luks(mapper, path).await
            }
            Self::LuksDevice {
                mapper,
                stage2_path: None,
            } => {
                // The mapper device *is* the root filesystem.
                self.mount_block(mapper).await
            }
        }
    }

    async fn extract_tarball(&self, path: &str) -> miette::Result<()> {
        info!(path, "Extracting stage-2 tarball to /real_root");

        let path_buf = Path::new(path);
        if !path_buf.exists() {
            return Err(miette!("stage2 tarball not found: {path}"));
        }

        // Extract synchronously in a blocking task (initramfs is small enough)
        smol::unblock({
            let path = path.to_string();
            move || -> miette::Result<()> { Self::extract_tarball_sync(&path) }
        })
        .await
    }

    fn extract_tarball_sync(path: &str) -> miette::Result<()> {
        // Use tar command-line tool for synchronous extraction
        let output = std::process::Command::new("tar")
            .args(["-xzf", path, "-C", REAL_ROOT])
            .output()
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to run tar for {path}"))?;

        if !output.status.success() {
            return Err(miette!(
                "tar extraction failed (exit {}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        info!("Stage-2 tarball extracted");
        Ok(())
    }

    async fn mount_block(&self, device: &str) -> miette::Result<()> {
        info!(device, "Mounting block device to /real_root");

        // Try common filesystems: ext4, ext3, ext2, btrfs, xfs
        let fstypes = ["ext4", "ext3", "ext2", "btrfs", "xfs"];
        let mut last_err = None;

        for fstype in &fstypes {
            let result = rustix::mount::mount(
                device,
                REAL_ROOT,
                *fstype,
                rustix::mount::MountFlags::empty(),
                None::<&std::ffi::CStr>,
            );

            if result.is_ok() {
                info!("Mounted {device} as {fstype}");
                return Ok(());
            }
            last_err = Some(format!("{fstype}: {result:?}"));
        }

        Err(miette!(
            "failed to mount {device} with any of {fstypes:?}: {last_err:?}"
        ))
    }

    async fn mount_and_extract_from_luks(
        &self,
        mapper: &str,
        stage2_path: &str,
    ) -> miette::Result<()> {
        // Mount the unlocked boot partition temporarily
        let boot_mount = "/boot_unlocked";
        smol::fs::create_dir_all(boot_mount)
            .await
            .into_diagnostic()?;

        info!(
            mapper,
            mount = boot_mount,
            "Mounting unlocked boot partition"
        );

        rustix::mount::mount(
            mapper,
            boot_mount,
            "ext4",
            rustix::mount::MountFlags::empty(),
            None::<&std::ffi::CStr>,
        )
        .into_diagnostic()
        .wrap_err("failed to mount unlocked boot partition")?;

        // Extract the stage2 tarball from inside the boot partition
        let full_path = format!("{boot_mount}/{stage2_path}");
        info!(path = full_path, "Extracting stage-2 from unlocked boot");
        self.extract_tarball(&full_path).await?;

        // Unmount the boot partition
        rustix::mount::unmount(
            boot_mount,
            rustix::mount::UnmountFlags::empty(),
        )
        .into_diagnostic()
        .wrap_err("failed to unmount boot partition")?;

        Ok(())
    }
}
