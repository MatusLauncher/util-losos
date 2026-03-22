use std::{
    env::args,
    fs,
    net::Ipv4Addr,
    path::{Path, PathBuf},
    process::{self, Command},
    str::FromStr,
    thread,
    time::Duration,
};

use actman::cmdline::CmdLineOptions;
use clap::Parser;
use miette::{IntoDiagnostic, bail, miette};
use rustyx::RustyX;
use serde_json::json;
use tracing::{error, info, warn};
use tracing_subscriber::fmt;

use crate::schemas::{ClientState, CluManSchema, Mode, ServerState, Task};

mod schemas;

const PORT: u16 = 9999;

// ── Controller CLI args ───────────────────────────────────────────────────────

/// Push Docker Compose files to cluster servers.
///
/// Reads each compose file from disk and forwards its full contents to every
/// listed server.  The controller exits once all pushes have completed.
#[derive(Debug, Parser)]
#[command(version, about)]
struct ControllerArgs {
    /// One or more Docker Compose files to push to the servers.
    #[arg(required = true)]
    compose_files: Vec<PathBuf>,

    /// Comma-separated server base-URLs to push tasks to.
    ///
    /// Example: `http://10.0.0.1:9999,http://10.0.0.2:9999`
    #[arg(
        short,
        long,
        env = "SERVER_URLS",
        value_delimiter = ',',
        required = true
    )]
    servers: Vec<String>,
}

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
        Mode::Controller => run_controller(ControllerArgs::parse()).await,
        // Server and client are boot-time daemons — configured via /proc/cmdline.
        Mode::Server => {
            let cmdline = CmdLineOptions::new()?;
            run_server(&cmdline).await
        }
        Mode::Client => {
            let cmdline = CmdLineOptions::new()?;
            run_client(&cmdline).await
        }
    }
}

// ── Controller ────────────────────────────────────────────────────────────────
//
// One-shot: reads compose files from disk, pushes their full contents to every
// listed server as Task objects, then exits.  It does NOT run a server.
//
// All configuration comes from clap (see ControllerArgs above).

async fn run_controller(args: ControllerArgs) -> miette::Result<()> {
    info!(
        servers = args.servers.len(),
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

        for url in &args.servers {
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

// ── Server ────────────────────────────────────────────────────────────────────
//
// Long-running.  Receives compose tasks pushed by controllers, queues them,
// and hands them out to clients on demand.
//
// /proc/cmdline keys:
//   own_ip=<ipv4>   (used in registration payload; defaults to 127.0.0.1)
//
// Exposes:
//   POST /api/push-task        — controllers push Task JSON here
//   POST /api/register-client  — clients register on start-up
//   GET  /task                 — clients claim the next pending Task (204 if empty)
//   GET  /clients              — introspection: list of registered client IPs
//   GET  /pending              — introspection: number of tasks in the queue

async fn run_server(cmdline: &CmdLineOptions) -> miette::Result<()> {
    let own_ip: Ipv4Addr = cmdline
        .opts()
        .get("own_ip")
        .and_then(|s| s.parse().ok())
        .unwrap_or(Ipv4Addr::LOCALHOST);

    info!(%own_ip, port = PORT, "Server starting");

    let state = ServerState::new();
    let server = RustyX::new();

    // ── POST /api/push-task — controllers push work here ─────────────────────
    let st = state.clone();
    server.post("/api/push-task", move |req, res| {
        let st = st.clone();
        async move {
            match req.json::<Task>() {
                Ok(task) => {
                    info!(filename = task.filename, "Task received from controller");
                    st.push_task(task);
                    res.created(json!({ "response": "Task queued" }))
                }
                Err(e) => res.bad_request(&e.to_string()),
            }
        }
    });

    // ── POST /api/register-client ─────────────────────────────────────────────
    let st = state.clone();
    server.post("/api/register-client", move |req, res| {
        let st = st.clone();
        async move {
            match req.json::<CluManSchema>() {
                Ok(schema) if schema.peer().map_or(false, |(_, m)| m == Mode::Client) => {
                    let (ip, _) = schema.peer().unwrap();
                    st.register_client(ip);
                    info!(%ip, "Client registered");
                    res.created(json!({ "response": "Successfully registered" }))
                }
                _ => res.bad_request("Invalid request. Only clients can register with servers."),
            }
        }
    });

    // ── GET /task — clients claim the next pending task ───────────────────────
    let st = state.clone();
    server.get("/task", move |_req, res| {
        let st = st.clone();
        async move {
            match st.claim_task() {
                Some(task) => {
                    info!(filename = task.filename, "Task dispatched to client");
                    res.json(serde_json::to_value(&task).unwrap_or_default())
                }
                None => res.no_content(),
            }
        }
    });

    // ── GET /clients — introspection ──────────────────────────────────────────
    let st = state.clone();
    server.get("/clients", move |_req, res| {
        let st = st.clone();
        async move {
            let addrs: Vec<String> = st.client_addrs().iter().map(|a| a.to_string()).collect();
            res.json(json!({ "clients": addrs }))
        }
    });

    // ── GET /pending — introspection ──────────────────────────────────────────
    let st = state.clone();
    server.get("/pending", move |_req, res| {
        let st = st.clone();
        async move { res.json(json!({ "pending": st.pending_count() })) }
    });

    info!(port = PORT, "Server listening");
    server.listen(PORT).await.into_diagnostic()
}

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

async fn run_client(cmdline: &CmdLineOptions) -> miette::Result<()> {
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
