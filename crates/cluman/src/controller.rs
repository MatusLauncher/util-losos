use std::{fs, path::PathBuf, str::FromStr};

use clap::Parser;
use miette::{IntoDiagnostic, bail};
use tracing::{error, info};

use crate::schemas::{IpRange, Task};

fn parse_ip_range(s: &str) -> Result<IpRange, String> {
    IpRange::from_str(s).map_err(|e| e.to_string())
}

// ── Controller CLI args ───────────────────────────────────────────────────────

/// Push Docker Compose files to cluster servers.
///
/// Reads each compose file from disk and forwards its full contents to every
/// listed server.  The controller exits once all pushes have completed.
#[derive(Debug, Parser)]
#[command(version, about)]
pub(crate) struct ControllerArgs {
    /// One or more Docker Compose files to push to the servers.
    #[arg(required = true)]
    compose_files: Vec<PathBuf>,

    /// IP ranges of servers to push tasks to.
    ///
    /// Accepts single IPs (`10.0.0.1`), CIDR notation (`10.0.0.0/24`), or
    /// dash ranges (`10.0.0.1-10.0.0.20`).  Comma-separate multiple values.
    ///
    /// Example: `--servers 10.0.0.1-10.0.0.5,10.0.1.0/24`
    #[arg(
        short,
        long,
        env = "SERVER_IPS",
        value_delimiter = ',',
        value_parser = parse_ip_range,
        required = true
    )]
    servers: Vec<IpRange>,

    /// Port that every server listens on.
    #[arg(short, long, env = "SERVER_PORT", default_value_t = 9999u16)]
    port: u16,
}

// ── Controller ────────────────────────────────────────────────────────────────
//
// One-shot: reads compose files from disk, pushes their full contents to every
// listed server as Task objects, then exits.  It does NOT run a server.
//
// All configuration comes from clap (see ControllerArgs above).

pub(crate) async fn run_controller(args: ControllerArgs) -> miette::Result<()> {
    let server_urls: Vec<String> = args
        .servers
        .iter()
        .flat_map(IpRange::hosts)
        .map(|ip| format!("http://{}:{}", ip, args.port))
        .collect();

    info!(
        targets = server_urls.len(),
        files = args.compose_files.len(),
        "Controller pushing tasks"
    );

    let mut errors: usize = 0;

    for file_path in &args.compose_files {
        let filename = file_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_path.to_string_lossy().into_owned());

        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                error!(path = %file_path.display(), error = %e, "Failed to read compose file — skipping");
                errors += 1;
                continue;
            }
        };

        let task = Task::new(filename.clone(), content);
        let body = serde_json::to_string(&task).into_diagnostic()?;

        for url in &server_urls {
            match minreq::post(format!("{url}/api/push-task"))
                .with_header("Content-Type", "application/json")
                .with_body(body.clone())
                .send()
            {
                Ok(resp) if resp.status_code == 201 => {
                    info!(filename, url, "Task accepted by server");
                }
                Ok(resp) => {
                    error!(
                        filename,
                        url,
                        status = resp.status_code,
                        body = resp.as_str().unwrap_or(""),
                        "Server rejected task"
                    );
                    errors += 1;
                }
                Err(e) => {
                    error!(filename, url, error = %e, "Failed to reach server");
                    errors += 1;
                }
            }
        }
    }

    if errors > 0 {
        bail!("{errors} error(s) occurred while pushing tasks — see logs above");
    }

    info!("All tasks pushed successfully");
    Ok(())
}
