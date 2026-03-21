//! Pre-boot filesystem mounting.
//!
//! Walks the root filesystem and mounts each discovered path as a filesystem
//! type of the same name (e.g. `proc` → `mount -t proc /proc`). Directories
//! used for persistent data (`/home`, `/etc`, `/bin`, `/sbin`) are skipped so
//! that the in-memory initramfs copies remain in place.

use std::process::Command;

use miette::IntoDiagnostic;
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
                .into_iter()
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    name != "home" && name != "etc" && name != "bin" && name != "sbin"
                })
                .filter_map(|e| e.ok())
                .map(|e| e.path().display().to_string())
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
    /// entry. Returns the first error encountered, if any.
    pub fn mount(&self) -> miette::Result<()> {
        self
            .mounts
            .iter()
            .try_for_each(|mount| -> miette::Result<()> {
                Ok({
                    info!("Mounting {mount} to /{mount}");
                    Command::new("mount")
                        .arg("-t")
                        .arg(mount)
                        .arg(format!("/{mount}"))
                        .spawn()
                        .into_diagnostic()?;
                })
            })
    }
}
