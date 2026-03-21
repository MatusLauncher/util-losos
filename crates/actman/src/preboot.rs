//! Pre-boot filesystem mounting.
//!
//! Mounts the standard virtual filesystems needed before userspace starts.
//! Each entry in [`VIRTUAL_FS`] maps a mountpoint name to its filesystem type;
//! entries are only attempted if the corresponding directory exists under `/`.

use miette::IntoDiagnostic;
use rustix::ffi::CStr;
use rustix::mount::{MountFlags, mount};
use tracing::info;

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
        self.mounts
            .iter()
            .try_for_each(|(name, fstype)| -> miette::Result<()> {
                info!("Mounting {fstype} to /{name}");
                mount(*name, format!("/{name}").as_str(), *fstype, MountFlags::empty(), None::<&CStr>)
                    .into_diagnostic()
            })
    }
}
