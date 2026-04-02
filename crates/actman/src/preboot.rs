//! Pre-boot filesystem mounting.
//!
//! Mounts the standard virtual filesystems needed before userspace starts.
//! Each entry in `VIRTUAL_FS` maps a mountpoint name to its filesystem type;
//! entries are only attempted if the corresponding directory exists under `/`.

use std::{ffi::CStr, fs::create_dir_all, path::{Path, PathBuf}};

use miette::IntoDiagnostic;
use rustix::mount::{MountFlags, mount};
use tracing::{info, warn};

use crate::{cmdline::CmdLineOptions, persistence::Persistence};

/// `(directory_name, filesystem_type)` pairs for the standard virtual
/// filesystems that must be mounted in the early boot environment.
pub(crate) const VIRTUAL_FS: &[(&str, &str)] = &[
    ("dev", "devtmpfs"),
    ("proc", "proc"),
    ("sys", "sysfs"),
    ("tmp", "tmpfs"),
];

/// Filesystem mounter for the early boot environment.
///
/// On construction, [`Preboot`] builds the list of virtual filesystems to
/// mount by intersecting `VIRTUAL_FS` with the directories that actually
/// exist under `/`.  Calling [`mount`](Preboot::mount) then issues one
/// `mount(2)` syscall per entry.
#[derive(Debug, Clone)]
pub struct Preboot {
    pub(crate) mounts: Vec<(&'static str, &'static str)>,
}

#[allow(trivial_bounds)]
impl Default for Preboot {
    fn default() -> Self {
        Self {
            mounts: VIRTUAL_FS
                .iter()
                .copied()
                .filter(|(name, _)| std::path::Path::new("/").join(name).is_dir())
                .collect(),
        }
    }
}

#[allow(trivial_bounds)]
impl Preboot {
    /// Creates a new [`Preboot`] by checking which virtual fs directories exist.
    pub fn new() -> Self {
        Self::default()
    }

    /// Mounts each discovered virtual filesystem via `mount(2)` and optionally
    /// sets up a persistent overlay for `/etc`.
    ///
    /// # Return value
    ///
    /// Returns `Ok(Some(Persistence))` when a `data_drive` is present and the
    /// `/etc` overlay was successfully mounted.  The caller **must** keep the
    /// returned value alive for as long as the overlay should remain mounted —
    /// dropping it unmounts the FUSE session via [`Persistence`]'s `Drop` impl.
    ///
    /// Returns `Ok(None)` when no `data_drive` is configured (RAM-only mode).
    pub fn mount(&self) -> miette::Result<Option<Persistence>> {
        // Mount virtual filesystems first — /proc must exist before we can
        // read /proc/cmdline below.
        self.mounts
            .iter()
            .try_for_each(|(name, fstype)| -> miette::Result<()> {
                info!("Mounting {fstype} to /{name}");
                mount(
                    *name,
                    format!("/{name}").as_str(),
                    *fstype,
                    MountFlags::empty(),
                    None::<&CStr>,
                )
                .into_diagnostic()
            })?;

        // Now /proc is mounted; check for an optional data drive.
        let drive = CmdLineOptions::new()?.opts().get("data_drive").cloned();
        match drive {
            Some(ref d) => {
                info!("Mounting the data drive to /data");
                create_dir_all("/data").into_diagnostic()?;
                mount(
                    d.as_str(),
                    Path::new("/data"),
                    "",
                    MountFlags::empty(),
                    None::<&CStr>,
                )
                .into_diagnostic()?;

                // Provide a durable overlay for /etc so configuration
                // changes written during this session persist across reboots.
                // The lower layer lives on the data drive; the upper layer
                // accumulates changes until actman exits and commits on
                // controlled shutdown.
                info!("Setting up persistent overlay for /etc");
                create_dir_all("/data/etc.lower").into_diagnostic()?;
                let mut etc_persist = Persistence::new(PathBuf::from("/data/etc.lower"));
                etc_persist.mount()?;
                Ok(Some(etc_persist))
            }
            None => {
                warn!("No data_drive kernel parameter set. The OS is running entirely in RAM.");
                Ok(None)
            }
        }
    }
}
