//! `pakman` library crate — package installation, removal, and execution.
//!
//! # Public surface
//!
//! | Item | Purpose |
//! |---|---|
//! | [`install`] | [`install::PackageInstallation`] — mounts the data drive and builds/saves Nix-based container images. |
//! | [`run`] | [`run::ProgRunner`] — loads a saved image tarball and runs it interactively. |
//!
//! Splitting the installation and run logic into separate, publicly-exported
//! modules means integration tests and benchmarks can import and exercise each
//! component independently without requiring `nerdctl` or a live data drive to
//! be present on the test host.

// ── Install ───────────────────────────────────────────────────────────────────
//
// Exposed as `pub mod` so that tests and benchmarks can import:
//
//   use pakman::install::PackageInstallation;

pub mod install;

// ── Run ───────────────────────────────────────────────────────────────────────
//
// Exposed as `pub mod` so that tests and benchmarks can import:
//
//   use pakman::run::ProgRunner;

pub mod run;
