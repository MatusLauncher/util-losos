use std::{env::args, path::Path, str::FromStr};

use actman::cmdline::CmdLineOptions;
use clap::Parser;
use tracing_subscriber::fmt;

use crate::schemas::Mode;

mod client;
mod controller;
mod schemas;
mod server;

pub(crate) const PORT: u16 = 9999;

// ── Entry point ───────────────────────────────────────────────────────────────

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
