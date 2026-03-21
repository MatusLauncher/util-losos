//! Pre-boot filesystem mounting.
//!
//! Walks the root filesystem and mounts each discovered path as a filesystem
//! type of the same name (e.g. `proc` → `mount -t proc /proc`). Directories
//! used for persistent data (`/home`, `/etc`, `/bin`, `/sbin`) are skipped so
//! that the in-memory initramfs copies remain in place.

use std::process::Command;

use miette::{IntoDiagnostic, miette};
use tracing::info;
use walkdir::WalkDir;

/// Filesystem mounter for the early boot environment.
///
/// On construction, [`Preboot`] discovers the mountable paths by walking `/`
/// and filtering out directories that should not be shadowed by a mount.
/// Calling [`mount`](Preboot::mount) then issues one `mount -t <name> /<name>`
/// command per discovered path.
#[derive(Debug, Clone)]
pub struct Preboot {
    /// Paths to mount, collected during [`Default::default`].
    mounts: Vec<String>,
}

#[allow(trivial_bounds)]
impl Default for Preboot {
    /// Walks `/`, excluding `home`, `etc`, `bin`, and `sbin`, and collects
    /// the remaining paths as mount targets.
    fn default() -> Self {
        Self {
            mounts: WalkDir::new("/")
                .max_depth(1)
                .min_depth(1)
                .into_iter()
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    name != "home" && name != "etc" && name != "bin" && name != "sbin"
                })
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_dir())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
        }
    }
}

#[allow(trivial_bounds)]
impl Preboot {
    /// Creates a new [`Preboot`] by walking the root filesystem.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mounts each discovered path by running `mount -t <path> /<path>`.
    ///
    /// Iterates over `self.mounts` and spawns a `mount` process for each
    /// entry. If the normal mount fails, retries with the `*fs` suffix
    /// (e.g., `procfs`, `sysfs`). For `tmpfs`, uses `devtmpfs` instead.
    /// Returns the first error encountered, if any.
    pub fn mount(&self) -> miette::Result<()> {

        self.mounts
            .iter()
            .try_for_each(|mount| -> miette::Result<()> {
                info!("Mounting {mount} to /{mount}");

                // Special case: tmpfs → devtmpfs
                let fstype = if mount == "tmpfs" { "devtmpfs" } else { mount };
                let result = Command::new("mount").arg("-t").arg(fstype).arg(mount).arg(format!("/{mount}")).status();

                if result.into_diagnostic()?.success() {
                    return Ok(());
                }

                // Fallback: try with *fs suffix
                let fs_suffix = format!("{mount}fs");
                info!("Mounting {mount} failed, retrying with {fs_suffix}");
                let result = Command::new("mount").arg("-t").arg(&fs_suffix).arg(mount).arg(format!("/{mount}")).status();

                if !result.into_diagnostic()?.success() {
                    return Err(miette!("Failed to mount {mount} (tried {mount} and {fs_suffix})"));
                }

                Ok(())
            })
    }
}
