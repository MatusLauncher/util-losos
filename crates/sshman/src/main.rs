use std::{env::args, path::Path};

use actman::cmdline::CmdLineOptions;
use tracing::info;
use tracing_subscriber::fmt;

use sshman::{mode::ModeOfOperation, server};

/// Entry point.
///
/// Reads the process name from `argv[0]`, converts it to a
/// [`ModeOfOperation`], and dispatches accordingly.
#[tokio::main]
async fn main() -> miette::Result<()> {
    let pname = Path::new(&args().collect::<Vec<_>>()[0])
        .file_name()
        .unwrap()
        .display()
        .to_string();
    fmt().init();

    let cmdline = CmdLineOptions::new()?;
    let userdb_addr = cmdline.opts().get("usvc_ip").cloned().unwrap_or_default();

    match ModeOfOperation::from(pname) {
        ModeOfOperation::Daemon => {
            let config = server::SshConfig::new(userdb_addr)?;
            server::run(config).await?;
        }
        ModeOfOperation::Unknown => {
            info!("Unknown mode. Symlink this binary as 'sshman' or 'sshd'.");
        }
    }

    Ok(())
}
