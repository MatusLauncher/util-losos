use std::{
    env::args,
    fs,
    net::Ipv4Addr,
    path::Path,
    process::{self, Command},
    str::FromStr,
    thread,
    time::Duration,
};

use actman::cmdline::CmdLineOptions;
use miette::{IntoDiagnostic, bail, miette};
use rustyx::RustyX;
use serde_json::json;
use tracing::{error, info, warn};
use tracing_subscriber::fmt;

use crate::schemas::{ClientState, CluManSchema, Mode, ServerState, Task};

mod schemas;

const PORT: u16 = 9999;

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> miette::Result<()> {
    fmt().init();

    // argv[0] determines the mode — the binary should be symlinked (or copied)
    // to `client`, `server`, or `controller` / `cluman`.
    let mut argv = args();
    let argv0 = argv.next().unwrap_or_default();
    // Remaining args are forwarded to the controller as compose file paths.
    let rest: Vec<String> = argv.collect();

    let mode = Mode::from_str(
        Path::new(&argv0)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .as_ref(),
    )?;

    let cmdline = CmdLineOptions::new()?;

    match mode {
        Mode::Controller => run_controller(&cmdline, &rest).await,
        Mode::Server => run_server(&cmdline).await,
        Mode::Client => run_client(&cmdline).await,
    }
}

// ── Controller ────────────────────────────────────────────────────────────────
//
// One-shot: reads compose files from disk, pushes their full contents to every
// listed server as Task objects, then exits.  It does NOT run a server.
//
// /proc/cmdline keys:
//   server_urls=http://HOST:PORT,...   (required — comma-separated)
//
// Compose files are taken from the remaining process arguments (argv[1..]).
// If none are provided, falls back to:
//   compose_files=/path/a.yml,...      (comma-separated in /proc/cmdline)

async fn run_controller(cmdline: &CmdLineOptions, compose_args: &[String]) -> miette::Result<()> {
    let server_urls: Vec<String> = cmdline
        .opts()
        .get("server_urls")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();

    if server_urls.is_empty() {
        bail!("No server URLs in /proc/cmdline — add server_urls=http://HOST:PORT,...");
    }

    // Compose files: prefer process argv, fall back to /proc/cmdline key.
    let compose_paths: Vec<String> = if !compose_args.is_empty() {
        compose_args.to_vec()
    } else {
        cmdline
            .opts()
            .get("compose_files")
            .map(|s| s.split(',').map(str::to_string).collect())
            .unwrap_or_default()
    };

    if compose_paths.is_empty() {
        bail!(
            "No compose files specified. \
             Pass them as arguments or set compose_files=... in /proc/cmdline."
        );
    }

    info!(
        servers = server_urls.len(),
        files = compose_paths.len(),
        "Controller pushing tasks"
    );

    let mut errors: usize = 0;

    for path_str in &compose_paths {
        let file_path = Path::new(path_str);
        let filename = file_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path_str.clone());

        let content = match fs::read_to_string(file_path) {
            Ok(c) => c,
            Err(e) => {
                error!(path = path_str, error = %e, "Failed to read compose file — skipping");
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
