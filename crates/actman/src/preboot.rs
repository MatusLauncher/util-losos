//! Pre-boot filesystem mounting.
//!
//! Mounts the standard virtual filesystems needed before userspace starts.
//! Each entry in [`VIRTUAL_FS`] maps a mountpoint name to its filesystem type;
//! entries are only attempted if the corresponding directory exists under `/`.

use std::fs::create_dir_all;
use std::path::Path;

use miette::IntoDiagnostic;
use rustix::ffi::CStr;
use rustix::mount::{MountFlags, mount};
use tracing::{info, warn};

use crate::cmdline::CmdLineOptions;

/// `(directory_name, filesystem_type)` pairs for the standard virtual
/// filesystems that must be mounted in the early boot environment.
const VIRTUAL_FS: &[(&str, &str)] = &[
    ("dev", "devtmpfs"),
    ("proc", "proc"),
    ("sys", "sysfs"),
    ("tmp", "tmpfs"),
];

/// Filesystem mounter for the early boot environment.
///
/// On construction, [`Preboot`] builds the list of virtual filesystems to
/// mount by intersecting [`VIRTUAL_FS`] with the directories that actually
/// exist under `/`.  Calling [`mount`](Preboot::mount) then issues one
/// `mount(2)` syscall per entry.
#[derive(Debug, Clone)]
pub struct Preboot {
    mounts: Vec<(&'static str, &'static str)>,
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

    /// Mounts each discovered virtual filesystem via `mount(2)`.
    pub fn mount(&self) -> miette::Result<()> {
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
                    String::new(),
                    MountFlags::empty(),
                    None::<&CStr>,
                )
                .into_diagnostic()?;
            }
            None => warn!("No data_drive kernel parameter set. The OS is running entirely in RAM."),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Preboot, VIRTUAL_FS};

    /// Every entry that `Preboot::default()` keeps must come from `VIRTUAL_FS`.
    #[test]
    fn mounts_is_subset_of_virtual_fs() {
        let preboot = Preboot::default();
        for entry in &preboot.mounts {
            assert!(
                VIRTUAL_FS.contains(entry),
                "mounts contains {entry:?} which is not in VIRTUAL_FS"
            );
        }
    }

    /// Every directory kept in `mounts` must actually exist under `/`.
    /// (This is the invariant that `Default` enforces via the `is_dir` filter.)
    #[test]
    fn mounts_entries_are_existing_directories() {
        let preboot = Preboot::default();
        for (name, _fstype) in &preboot.mounts {
            let path = std::path::Path::new("/").join(name);
            assert!(
                path.is_dir(),
                "/{name} should be a directory but is not present in this environment"
            );
        }
    }

    /// Entries from `VIRTUAL_FS` whose directories do not exist must be absent
    /// from the constructed `mounts` list.
    #[test]
    fn missing_directories_are_excluded() {
        let preboot = Preboot::default();
        for (name, _fstype) in VIRTUAL_FS {
            let exists = std::path::Path::new("/").join(name).is_dir();
            let in_mounts = preboot.mounts.iter().any(|(n, _)| n == name);
            assert_eq!(
                exists, in_mounts,
                "/{name}: exists={exists} but in_mounts={in_mounts} — filter is inconsistent"
            );
        }
    }

    /// `Preboot::new()` and `Preboot::default()` must produce identical results.
    #[test]
    fn new_and_default_are_equivalent() {
        let via_new = Preboot::new();
        let via_default = Preboot::default();
        assert_eq!(
            via_new.mounts, via_default.mounts,
            "Preboot::new() and Preboot::default() should produce the same mounts list"
        );
    }
}
