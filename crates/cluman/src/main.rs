//! Entry point for the `cluman` binary — dispatches to client, server, or controller mode based on `argv[0]`.

use std::{env::args, path::Path, str::FromStr};

// Re-export the library's schemas module so that `crate::schemas` resolves
// for the client / server / controller sub-modules declared below.
mod schemas {
    pub use cluman::schemas::*;
}

use actman::cmdline::CmdLineOptions;
use clap::Parser;
use tracing_subscriber::fmt;

use cluman::schemas::Mode;

mod client;
mod controller;
mod server;

/// Default TCP port that clients and servers listen on.
pub(crate) const PORT: u16 = 9999;

// ── Entry point ───────────────────────────────────────────────────────────────

/// Initialises tracing, determines the mode from `argv[0]`, and dispatches to the appropriate runtime (`run_controller`, `run_server`, or `run_client`).
#[tokio::main]
async fn main() -> miette::Result<()> {
    fmt().init();

    // argv[0] determines the mode — the binary should be symlinked (or copied)
    // to `client`, `server`, or `controller` / `cluman`.
    let argv0 = args().next().unwrap_or_default();
    let mode = Mode::from_str(
        Path::new(&argv0)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .as_ref(),
    )?;

    match mode {
        // Controller is a one-shot CLI tool — configured entirely via clap.
        Mode::Controller => controller::run_controller(controller::ControllerArgs::parse()).await,
        // Server and client are boot-time daemons — configured via /proc/cmdline.
        Mode::Server => {
            let cmdline = CmdLineOptions::new()?;
            server::run_server(&cmdline).await
        }
        Mode::Client => {
            let cmdline = CmdLineOptions::new()?;
            client::run_client(&cmdline).await
        }
    }
}
