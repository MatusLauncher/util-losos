//! # pakman — Package Manager
//!
//! `pakman` is a minimal package manager for the `util-mdl` initramfs OS. It
//! installs programs on demand by building NixOS-based container images with
//! [`nerdctl`](https://github.com/containerd/nerdctl), persisting them as
//! tarballs on a data drive, and running them inside isolated containers.
//!
//! ## Usage
//!
//! ```text
//! pakman --install <pkg> [<pkg> ...]   # install one or more packages
//! pakman --remove  <pkg> [<pkg> ...]   # remove installed packages
//! pakman --run     <pkg>               # run an installed package
//! ```
//!
//! ## Install flow
//!
//! 1. Reads the `data_drive` key from `/proc/cmdline` via
//!    [`actman::cmdline::CmdLineOptions`].
//! 2. Mounts the data drive to `/data` if it is not already mounted.
//! 3. Ensures `/data/progs/` exists.
//! 4. For each package, writes a minimal `Dockerfile`, builds a container
//!    image tagged `local/<pkg>`, and saves it to `/data/progs/<pkg>.tar`.
//!    All packages are processed in parallel via [`std::thread::scope`].
//!
//! ## Remove flow
//!
//! Deletes `/data/progs/<pkg>.tar` for each named package.
//!
//! ## Run flow
//!
//! 1. Walks `/data/progs/` to find the tarball whose name contains `<pkg>`.
//! 2. Loads it into the container runtime with `nerdctl load`.
//! 3. Runs it interactively with `nerdctl run -it`.
//!
//! ## Configuration
//!
//! `pakman` requires the following kernel command-line parameter for install
//! and remove operations:
//!
//! | Key          | Description                                          |
//! |--------------|------------------------------------------------------|
//! | `data_drive` | Block device path for the persistent data partition  |
//!
//! Example: `data_drive=/dev/sda2`
//!
//! ## Requirements
//!
//! - `nerdctl` must be available at `/bin/nerdctl`.
//! - `data_drive` must be set in the kernel command line before using
//!   `--install` or `--remove`.

use std::{fs::remove_file, path::Path};

use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::fmt;

use pakman::install::PackageInstallation;
use pakman::run::ProgRunner;

/// Command-line interface for `pakman`.
///
/// Exactly one of `--install`, `--remove`, or `--run` should be provided per
/// invocation. If none are given, a warning is printed and the process exits
/// successfully.
///
/// # Examples
///
/// ```text
/// # Install curl and git
/// pakman --install curl git
///
/// # Remove curl
/// pakman --remove curl
///
/// # Run git interactively
/// pakman --run git
/// ```
#[derive(Parser)]
#[clap(name = "pakman", version, about, long_about = None)]
pub struct CLIface {
    /// One or more package names to install.
    ///
    /// For each name a container image is built from a `nixos/nix` base and
    /// saved to `/data/progs/<pkg>.tar`. All packages are built in parallel.
    #[arg(long = "install")]
    install: Option<Vec<String>>,

    /// One or more package names to remove.
    ///
    /// Deletes the corresponding tarball(s) from `/data/progs/`. The package
    /// will no longer be available to `--run` after removal.
    #[arg(long = "remove")]
    remove: Option<Vec<String>>,

    /// Name of the package to run.
    ///
    /// Finds the tarball in `/data/progs/`, loads it into the container
    /// runtime, and starts it interactively.
    #[arg(long = "run")]
    run: Option<String>,
}

/// Entry point for `pakman`.
///
/// Initialises the [`tracing`] subscriber, parses CLI arguments via
/// [`CLIface`], and dispatches to the appropriate operation:
///
/// - `--install` → [`PackageInstallation`]
/// - `--remove`  → [`std::fs::remove_file`] for each named tarball
/// - `--run`     → [`ProgRunner`]
///
/// Returns a [`miette::Result`] so that errors are printed with full
/// diagnostic context on failure.
fn main() -> miette::Result<()> {
    fmt().init();
    let args = CLIface::parse();
    let mut installation = PackageInstallation::new();
    if let Some(i) = args.install {
        i.iter().for_each(|prog| installation.add_to_queue(prog));
        installation.start()?;
    } else if let Some(rm) = args.remove {
        rm.iter().for_each(|to_rm| {
            info!("Removing {to_rm}");
            remove_file(Path::new("/data/progs").join(format!("{to_rm}.tar"))).unwrap();
        });
    } else if let Some(run) = args.run {
        let pr = ProgRunner::new();
        info!("Running {run}");
        pr.run(&run)?;
    } else {
        warn!("Run pakman --help for help.");
    }
    Ok(())
}
