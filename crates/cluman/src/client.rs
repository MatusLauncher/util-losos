use std::{
    fs,
    net::Ipv4Addr,
    process::{self, Command},
    thread,
    time::Duration,
};

use actman::cmdline::CmdLineOptions;
use miette::{IntoDiagnostic, miette};
use rustyx::RustyX;
use serde_json::json;
use tracing::{error, info, warn};

use crate::{
    PORT,
    schemas::{ClientState, CluManSchema, Mode, Task},
};

// ── Client ────────────────────────────────────────────────────────────────────
//
// Long-running.  Registers with a server on start-up, then polls it for tasks.
// Each received Task has its compose file content written to a temp file,
// executed via `docker compose`, and cleaned up afterwards.
//
// /proc/cmdline keys:
//   server_url=http://HOST:PORT   (required)
//   own_ip=<ipv4>                 (used in registration payload)
//
// Exposes:
//   GET /status   — health check

pub(crate) async fn run_client(cmdline: &CmdLineOptions) -> miette::Result<()> {
    let server_url = cmdline.opts().get("server_url").cloned().ok_or_else(|| {
        miette!("No server URL in /proc/cmdline — add server_url=http://HOST:PORT")
    })?;

    let own_ip: Ipv4Addr = cmdline
        .opts()
        .get("own_ip")
        .and_then(|s| s.parse().ok())
        .unwrap_or(Ipv4Addr::LOCALHOST);

    info!(%own_ip, server_url, "Client starting");

    let state = ClientState::new(server_url.clone(), own_ip);
    let server = RustyX::new();

    // ── Register with the server ──────────────────────────────────────────────
    let payload = serde_json::to_string(&CluManSchema::registration(own_ip, Mode::Client))
        .into_diagnostic()?;

    match minreq::post(format!("{server_url}/api/register-client"))
        .with_header("Content-Type", "application/json")
        .with_body(payload)
        .send()
    {
        Ok(resp) => info!(status = resp.status_code, "Registered with server"),
        Err(e) => warn!(error = %e, "Failed to register with server — will retry next poll"),
    }

    // ── Background: poll server for tasks every 10 s ──────────────────────────
    // minreq is synchronous; keep this in a plain OS thread.
    let st = state.clone();
    thread::spawn(move || {
        loop {
            match minreq::get(format!("{}/task", st.server_url)).send() {
                Ok(resp) if resp.status_code == 200 => {
                    if let Ok(body) = resp.as_str() {
                        match serde_json::from_str::<Task>(body) {
                            Ok(task) => {
                                info!(filename = task.filename, "Received task from server");
                                execute_compose(task);
                            }
                            Err(e) => warn!(error = %e, "Failed to parse task from server"),
                        }
                    }
                }
                Ok(resp) if resp.status_code == 204 => {} // no tasks yet — normal
                Ok(resp) => warn!(status = resp.status_code, "Unexpected status polling task"),
                Err(e) => warn!(error = %e, "Failed to poll server for task"),
            }
            thread::sleep(Duration::from_secs(10));
        }
    });

    // ── GET /status — health check ────────────────────────────────────────────
    server.get("/status", move |_req, res| async move {
        res.json(json!({ "status": "ok", "mode": "client" }))
    });

    info!(port = PORT, "Client listening");
    server.listen(PORT).await.into_diagnostic()
}

// ── Task execution ────────────────────────────────────────────────────────────

/// Write the task's compose file content to a temp file, run
/// `docker compose -f <tmp> up -d --remove-orphans`, then delete the temp file.
///
/// Runs synchronously inside the polling thread so at most one compose project
/// is started at a time per client node.
fn execute_compose(task: Task) {
    let tmp_path = std::env::temp_dir().join(format!("cluman-{}-{}", process::id(), task.filename));

    if let Err(e) = fs::write(&tmp_path, &task.content) {
        error!(
            filename = task.filename,
            path     = %tmp_path.display(),
            error    = %e,
            "Failed to write compose file to temp path"
        );
        return;
    }

    let path_str = tmp_path.to_string_lossy().into_owned();

    match Command::new("docker")
        .args(["compose", "-f", &path_str, "up", "-d", "--remove-orphans"])
        .output()
    {
        Ok(out) if out.status.success() => {
            info!(filename = task.filename, "docker compose up succeeded");
        }
        Ok(out) => {
            error!(
                filename  = task.filename,
                exit_code = out.status.code().unwrap_or(-1),
                stderr    = %String::from_utf8_lossy(&out.stderr),
                "docker compose up failed",
            );
        }
        Err(e) => {
            error!(filename = task.filename, error = %e, "Failed to spawn docker compose");
        }
    }

    if let Err(e) = fs::remove_file(&tmp_path) {
        warn!(
            path  = %tmp_path.display(),
            error = %e,
            "Failed to remove temp compose file"
        );
    }
}
