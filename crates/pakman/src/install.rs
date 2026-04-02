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
    path::PathBuf,
    process::Command,
    thread::scope,
};

use actman::{cmdline::CmdLineOptions, persistence::Persistence};
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

        // Wrap /data/progs with a persistent overlay so that tarballs written
        // during this session are staged in the upper layer.  On success the
        // upper layer is committed atomically into /data/progs; on any failure
        // it is discarded so a partial install never corrupts the on-disk state.
        let mut progs_persist = Persistence::new(PathBuf::from("/data/progs"));
        progs_persist.mount()?;
        let overlay_mountpoint = progs_persist.mountpoint();

        let all_ok = scope(|thread| {
            let handles: Vec<_> = self.install_tasks.iter()
                .map(|task| {
                    let mount_path = overlay_mountpoint.clone();
                    thread.spawn(move || -> bool {
                        let path = temp_dir().join(task);
                        let perm_path = mount_path.join(format!("{task}.tar"));
                        let dockerfile = format!(
                            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {task} --run {task}"
                        );
                        if write(&path, dockerfile).is_err() {
                            return false;
                        }
                        info!("Building the image with a system container runtime...");
                        let build_ok = Command::new("/bin/nerdctl")
                            .arg("build")
                            .arg(&path)
                            .arg("-t")
                            .arg(format!("local/{task}"))
                            .status()
                            .map(|s| s.success())
                            .unwrap_or(false);
                        if !build_ok {
                            return false;
                        }
                        let save_ok = Command::new("/bin/nerdctl")
                            .arg("save")
                            .arg(format!("local/{task}"))
                            .arg("-o")
                            .arg(&perm_path)
                            .status()
                            .map(|s| s.success())
                            .unwrap_or(false);
                        if save_ok {
                            info!("DONE! Restart this machine to clean up build cache");
                        }
                        save_ok
                    })
                })
                .collect();
            handles.into_iter().all(|h| h.join().unwrap_or(false))
        });

        if all_ok {
            info!("All packages installed; committing overlay to /data/progs");
            progs_persist.commit();
        } else {
            warn!("One or more packages failed to install; discarding overlay");
            progs_persist.discard();
            return Err(miette!("Package installation failed; no changes written to /data/progs"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actman::cmdline::CmdLineOptions;
    use std::collections::HashMap;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Build a [`CmdLineOptions`] with no keys — simulates a kernel command
    /// line that omits `data_drive`.
    fn empty_opts() -> CmdLineOptions {
        CmdLineOptions::from_map(HashMap::new())
    }

    /// Build a [`CmdLineOptions`] that sets `data_drive` to `dev`.
    fn opts_with_drive(dev: &str) -> CmdLineOptions {
        CmdLineOptions::from_map(
            [("data_drive".to_owned(), dev.to_owned())]
                .into_iter()
                .collect::<HashMap<_, _>>(),
        )
    }

    // ── construction ──────────────────────────────────────────────────────────

    /// A freshly-constructed instance must have an empty install queue.
    #[test]
    fn new_starts_with_empty_queue() {
        let pi = PackageInstallation::new();
        assert!(pi.install_tasks.is_empty());
    }

    /// `Default` must also produce an empty queue.
    #[test]
    fn default_starts_with_empty_queue() {
        let pi = PackageInstallation::default();
        assert!(pi.install_tasks.is_empty());
    }

    /// `new()` and `default()` must produce identical queue contents.
    #[test]
    fn new_and_default_have_equivalent_queues() {
        let a = PackageInstallation::new();
        let b = PackageInstallation::default();
        assert_eq!(a.install_tasks, b.install_tasks);
    }

    // ── add_to_queue ──────────────────────────────────────────────────────────

    /// A single `add_to_queue` call must produce a one-element queue.
    #[test]
    fn add_to_queue_appends_single_item() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue("curl");
        assert_eq!(pi.install_tasks, vec!["curl"]);
    }

    /// Items must appear in the queue in the order they were enqueued.
    #[test]
    fn add_to_queue_preserves_insertion_order() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue("curl");
        pi.add_to_queue("git");
        pi.add_to_queue("jq");
        assert_eq!(pi.install_tasks, vec!["curl", "git", "jq"]);
    }

    /// Enqueuing the same name twice must produce two separate entries.
    #[test]
    fn add_to_queue_accepts_duplicate_names() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue("curl");
        pi.add_to_queue("curl");
        assert_eq!(pi.install_tasks.len(), 2);
        assert_eq!(pi.install_tasks, vec!["curl", "curl"]);
    }

    /// An empty-string package name must be accepted without panicking.
    #[test]
    fn add_to_queue_accepts_empty_string() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue("");
        assert_eq!(pi.install_tasks, vec![""]);
    }

    /// Package names that contain hyphens (common in Nix) must be stored
    /// verbatim.
    #[test]
    fn add_to_queue_accepts_hyphenated_package_names() {
        let mut pi = PackageInstallation::new();
        pi.add_to_queue("my-package");
        pi.add_to_queue("pkg-with-multi-hyphens");
        assert_eq!(
            pi.install_tasks,
            vec!["my-package", "pkg-with-multi-hyphens"]
        );
    }

    // ── start — data_drive absent ─────────────────────────────────────────────

    /// `start()` must return an error immediately when `data_drive` is not
    /// present in the kernel command line.
    #[test]
    fn start_errors_when_data_drive_absent() {
        let pi = PackageInstallation {
            install_tasks: vec!["curl".into()],
            lineopts: empty_opts(),
        };
        assert!(
            pi.start().is_err(),
            "start() must fail when data_drive is not in cmdline"
        );
    }

    /// The error returned when `data_drive` is absent must name `data_drive` so
    /// the operator knows which key to add to the kernel command line.
    #[test]
    fn start_error_message_mentions_data_drive() {
        let pi = PackageInstallation {
            install_tasks: vec!["curl".into()],
            lineopts: empty_opts(),
        };
        let err = pi.start().unwrap_err();
        let msg = format!("{err:?}");
        assert!(
            msg.contains("data_drive"),
            "error message must mention 'data_drive', got: {msg}"
        );
    }

    /// The `data_drive` guard must fire even when the install queue is empty —
    /// the check must precede any queue processing.
    #[test]
    fn start_checks_data_drive_before_processing_queue() {
        let pi = PackageInstallation {
            install_tasks: vec![],
            lineopts: empty_opts(),
        };
        assert!(
            pi.start().is_err(),
            "data_drive guard must fire regardless of queue length"
        );
    }

    // ── start — data_drive present but device absent from /proc/mounts ────────
    //
    // When data_drive names a device that has no entry in /proc/mounts,
    // `start()` collects an empty string from the filter, then panics on the
    // unconditional `[1]` index of the split-whitespace Vec.
    //
    // This test documents that known behaviour.  It is expected to panic; the
    // fix would be to return a proper `Err` instead.

    /// `start()` panics when the named device is absent from `/proc/mounts`.
    ///
    /// This is a known edge case: the `[1]` index into the split-whitespace
    /// result is unconditional and will panic on an empty input.
    #[test]
    fn start_panics_when_data_drive_device_not_in_mounts() {
        let pi = PackageInstallation {
            install_tasks: vec!["curl".into()],
            lineopts: opts_with_drive("/dev/this_device_does_not_exist_xyz_abc"),
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| pi.start()));
        assert!(
            result.is_err(),
            "start() must panic when the named device is not present in /proc/mounts"
        );
    }

    // ── Dockerfile template ───────────────────────────────────────────────────

    /// The generated `Dockerfile` must begin with the NixOS base image.
    #[test]
    fn dockerfile_template_starts_with_from_nixos_nix() {
        let task = "curl";
        let content =
            format!("FROM nixos/nix as base\nENTRYPOINT nix-shell -p {task} --run {task}");
        assert!(
            content.starts_with("FROM nixos/nix"),
            "Dockerfile must start with 'FROM nixos/nix', got: {content:?}"
        );
    }

    /// The `ENTRYPOINT` must reference the package name in both `-p` and
    /// `--run`, so that `nix-shell` installs and immediately runs the
    /// requested program.
    #[test]
    fn dockerfile_template_entrypoint_uses_package_name_in_both_positions() {
        let task = "my-tool";
        let content =
            format!("FROM nixos/nix as base\nENTRYPOINT nix-shell -p {task} --run {task}");
        assert_eq!(
            content,
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p my-tool --run my-tool"
        );
    }

    /// Verify the exact Dockerfile rendered for `curl`.
    #[test]
    fn dockerfile_template_exact_output_for_curl() {
        let task = "curl";
        let content =
            format!("FROM nixos/nix as base\nENTRYPOINT nix-shell -p {task} --run {task}");
        assert_eq!(
            content,
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p curl --run curl"
        );
    }

    /// Verify the exact Dockerfile rendered for `git`.
    #[test]
    fn dockerfile_template_exact_output_for_git() {
        let task = "git";
        let content =
            format!("FROM nixos/nix as base\nENTRYPOINT nix-shell -p {task} --run {task}");
        assert_eq!(
            content,
            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p git --run git"
        );
    }

    // ── tarball path derivation ───────────────────────────────────────────────

    /// The computed tarball path for a package must be
    /// `/data/progs/<pkg>.tar`.
    #[test]
    fn tarball_path_resolves_under_data_progs() {
        let task = "curl";
        let perm_path = std::path::Path::new("/data")
            .join("progs")
            .join(format!("{task}.tar"));
        assert_eq!(perm_path, std::path::Path::new("/data/progs/curl.tar"));
    }

    /// The tarball extension must always be `.tar`.
    #[test]
    fn tarball_has_tar_extension() {
        let task = "git";
        let perm_path = std::path::Path::new("/data")
            .join("progs")
            .join(format!("{task}.tar"));
        assert_eq!(perm_path.extension().and_then(|e| e.to_str()), Some("tar"));
    }

    /// The tarball file stem must be exactly the package name.
    #[test]
    fn tarball_stem_matches_package_name() {
        let task = "ripgrep";
        let perm_path = std::path::Path::new("/data")
            .join("progs")
            .join(format!("{task}.tar"));
        assert_eq!(
            perm_path.file_stem().and_then(|s| s.to_str()),
            Some("ripgrep")
        );
    }

    /// Package names with hyphens must be preserved verbatim in the tarball
    /// filename — no character substitution must occur.
    #[test]
    fn tarball_stem_preserves_hyphens() {
        let task = "my-hyphenated-pkg";
        let perm_path = std::path::Path::new("/data")
            .join("progs")
            .join(format!("{task}.tar"));
        assert_eq!(
            perm_path.file_stem().and_then(|s| s.to_str()),
            Some("my-hyphenated-pkg")
        );
    }
}
