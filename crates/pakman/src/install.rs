//! Package installation logic for `pakman`.
//!
//! This module provides [`PackageInstallation`], which is responsible for:
//!
//! 1. Verifying that a `data_drive` is specified in the kernel command line.
//! 2. Mounting that drive to `/data` if it is not already mounted.
//! 3. Building a minimal NixOS-based container image for each requested
//!    package using `nerdctl build`.
//! 4. Saving each built image as a tarball under `/data/progs/<pkg>.tar`
//!    using `nerdctl save`, so it persists across reboots.
//!
//! Each package is processed concurrently in its own thread via
//! [`std::thread::scope`], keeping install time proportional to the slowest
//! single package rather than the sum of all packages.
//!
//! # Kernel command line
//!
//! `pakman` discovers the data drive by reading `/proc/cmdline` through
//! [`CmdLineOptions`].  The required key is:
//!
//! ```text
//! data_drive=/dev/sda2
//! ```
//!
//! If the key is absent, [`PackageInstallation::start`] returns an error
//! immediately without attempting any installation.
//!
//! # Generated Dockerfile
//!
//! For a package named `curl` the following `Dockerfile` is written to a
//! temporary file before calling `nerdctl build`:
//!
//! ```text
//! FROM nixos/nix as base
//! ENTRYPOINT nix-shell -p curl --run curl
//! ```
//!
//! The resulting image is tagged `local/curl` and saved to
//! `/data/progs/curl.tar`.

use std::{
    env::temp_dir,
    fs::{create_dir_all, read_to_string, write},
    path::Path,
    process::Command,
    thread::scope,
};

use actman::cmdline::CmdLineOptions;
use miette::{IntoDiagnostic, miette};
use rustix::mount::{MountFlags, mount};
use tracing::{info, warn};

/// Manages a queue of packages to be installed onto the persistent data drive.
///
/// Create an instance with [`PackageInstallation::new`], enqueue packages with
/// [`PackageInstallation::add_to_queue`], and then call
/// [`PackageInstallation::start`] to begin the installation.
///
/// # Example
///
/// ```no_run
/// use pakman::install::PackageInstallation;
///
/// let mut installation = PackageInstallation::new();
/// installation.add_to_queue("curl");
/// installation.add_to_queue("git");
/// installation.start().expect("installation failed");
/// ```
#[derive(Default)]
pub struct PackageInstallation {
    /// Ordered list of package names waiting to be installed.
    install_tasks: Vec<String>,
    /// Parsed kernel command-line options, used to look up `data_drive`.
    lineopts: CmdLineOptions,
}

impl PackageInstallation {
    /// Creates a new, empty [`PackageInstallation`].
    ///
    /// The install queue is empty; populate it with
    /// [`add_to_queue`](Self::add_to_queue) before calling
    /// [`start`](Self::start).
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `pkg` to the install queue.
    ///
    /// Packages are processed in the order they are added, though the actual
    /// build and save steps run in parallel once [`start`](Self::start) is
    /// called.
    ///
    /// # Arguments
    ///
    /// * `pkg` — the name of the Nix package to install (e.g. `"curl"`).
    pub fn add_to_queue(&mut self, pkg: &str) {
        self.install_tasks.push(pkg.into());
    }

    /// Mounts the data drive and installs all queued packages.
    ///
    /// # Steps
    ///
    /// 1. Reads the `data_drive` key from `/proc/cmdline`.  Returns an error
    ///    if the key is not set.
    /// 2. Reads `/proc/mounts` and checks whether the device is already
    ///    mounted.  If it is not, mounts it at `/data`.
    /// 3. Creates `/data/progs/` if it does not exist.
    /// 4. Spawns one thread per queued package (via [`std::thread::scope`]).
    ///    Each thread:
    ///    - Writes a two-line `Dockerfile` to `$TMPDIR/<pkg>`.
    ///    - Calls `nerdctl build` to produce the image `local/<pkg>`.
    ///    - Calls `nerdctl save` to write the image to
    ///      `/data/progs/<pkg>.tar`.
    ///
    /// # Errors
    ///
    /// Returns a [`miette::Report`] if:
    ///
    /// - `data_drive` is absent from the kernel command line.
    /// - `/proc/mounts` cannot be read.
    /// - The mount syscall fails (via [`rustix::mount::mount`]).
    /// - Writing the temporary `Dockerfile` fails.
    /// - `nerdctl build` or `nerdctl save` cannot be spawned or fails.
    ///
    /// # Panics
    ///
    /// Will panic if `/proc/mounts` contains a line with fewer than two
    /// whitespace-separated fields — this should never happen on a healthy
    /// Linux system.
    pub fn start(&self) -> miette::Result<()> {
        info!("Checking whether data drive is mounted for persistency");
        let ddrive = self.lineopts.opts().get("data_drive");
        if ddrive.is_none() {
            return Err(miette!("Cannot continue, no data_drive is set."));
        }
        let mounts = read_to_string("/proc/mounts").into_diagnostic()?;
        let actual_mount = mounts
            .lines()
            .filter(|d| d.contains(self.lineopts.opts().get("data_drive").unwrap()))
            .map(|drive| drive.to_string())
            .collect::<String>();
        let mount_dir = actual_mount.split_whitespace().collect::<Vec<_>>()[1];
        if !mount_dir.starts_with("/") {
            info!("Mounting {} to /data", ddrive.unwrap());
            mount(ddrive.unwrap(), "/data", "", MountFlags::all(), None).into_diagnostic()?;
        } else {
            warn!("{} is already mounted, continuing", ddrive.unwrap());
        }
        match create_dir_all("/data/progs").into_diagnostic() {
            Ok(_) => (),
            Err(_) => warn!("The program directory probably exists, continuing"),
        };
        scope(|thread| {
            self.install_tasks.iter().for_each(|task| {
                thread.spawn(move || -> miette::Result<()> {
                    let path = temp_dir().join(task);
                    let join = Path::new("/data").join("progs");
                    let perm_path = join.join(format!("{task}.tar"));
                    write(
                        &path,
                        format!(
                            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {task} --run {task}"
                        ),
                    )
                    .into_diagnostic()?;
                    info!("Building the image with a system container runtime...");
                    let mut command = Command::new("/bin/nerdctl");
                    command
                        .arg("build")
                        .arg(&path)
                        .arg("-t")
                        .arg(format!("local/{task}"))
                        .status()
                        .into_diagnostic()?;
                    command
                        .arg("save")
                        .arg(format!("local/{task}"))
                        .arg("-o")
                        .arg(perm_path)
                        .spawn()
                        .into_diagnostic()?;
                    info!("DONE! Restart this machine to clean up build cache");
                    Ok(())
                });
            });
        });
        Ok(())
    }
}
