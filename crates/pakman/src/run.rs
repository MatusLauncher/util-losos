//! Program runner — loads a saved container image tarball and runs it interactively.
//!
//! After a package has been installed by [`crate::install::PackageInstallation`], its
//! container image is stored as a tarball under `/data/progs/<pkg>.tar`. [`ProgRunner`]
//! locates that tarball, feeds it to `nerdctl load`, and then launches the resulting
//! image with `nerdctl run -it`.
//!
//! # Flow
//!
//! ```text
//! ProgRunner::run("git")
//!     ├─ WalkDir /data/progs/   — find the first entry whose path contains "git"
//!     ├─ nerdctl load -i /data/progs/git.tar
//!     └─ nerdctl run -it localhost/local/git
//! ```
//!
//! # Requirements
//!
//! * `nerdctl` must be present at `/bin/nerdctl`.
//! * The requested package must have been previously installed with
//!   [`crate::install::PackageInstallation`] so that its tarball exists under
//!   `/data/progs/`.

use std::process::Command;

use miette::IntoDiagnostic;
use tracing::info;
use walkdir::WalkDir;

/// Loads and runs a previously installed program from its saved container image tarball.
///
/// `ProgRunner` is a stateless helper — all state lives on the filesystem under
/// `/data/progs/`. Construct one with [`ProgRunner::new`] and call [`ProgRunner::run`]
/// with the name of the program to launch.
///
/// # Example
///
/// ```rust,no_run
/// use pakman::run::ProgRunner;
///
/// let runner = ProgRunner::new();
/// runner.run("git").expect("failed to run git");
/// ```
#[derive(Default)]
pub struct ProgRunner;

impl ProgRunner {
    /// Creates a new `ProgRunner`.
    ///
    /// This is a zero-cost constructor — `ProgRunner` carries no state of its own.
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads the saved tarball for `prog` and runs it interactively.
    ///
    /// # Steps
    ///
    /// 1. Walks `/data/progs/` to find the first entry whose path contains `prog`.
    /// 2. Calls `nerdctl load -i <tarball>` to import the image into the container
    ///    runtime.
    /// 3. Calls `nerdctl run -it localhost/local/<prog>` to start the container.
    ///
    /// # Errors
    ///
    /// Returns a [`miette::Report`] if:
    ///
    /// * `nerdctl load` fails to start or returns a non-zero exit status.
    /// * No tarball matching `prog` is found under `/data/progs/` (the `WalkDir`
    ///   iterator will be empty and the index operation will panic — callers should
    ///   ensure the package is installed before calling this method).
    ///
    /// # Panics
    ///
    /// Panics if no entry matching `prog` exists in `/data/progs/`. Use
    /// [`crate::install::PackageInstallation`] to install the package first.
    pub fn run(&self, prog: &str) -> miette::Result<()> {
        info!("Loading the program from the data_drive");
        let p = WalkDir::new("/data/progs")
            .into_iter()
            .filter(|p| {
                let dir_entry = p.as_ref().unwrap();
                dir_entry.path().display().to_string().contains(prog)
            })
            .map(|fname| {
                let dir_entry = fname.as_ref();
                let fname = dir_entry.unwrap().path();
                fname.display().to_string()
            })
            .collect::<Vec<_>>()[0]
            .clone();
        Command::new("/bin/nerdctl")
            .arg("load")
            .arg("-i")
            .arg(p)
            .status()
            .into_diagnostic()?;
        info!("Starting up {prog}");
        Command::new("/bin/nerdctl")
            .arg("run")
            .arg("-it")
            .arg(format!("localhost/local/{prog}"));
        Ok(())
    }
}
