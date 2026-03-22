use std::{
    env::args, net::Ipv4Addr, path::Path, process::Command, str::FromStr, thread, time::Duration,
};

use actman::cmdline::CmdLineOptions;
use miette::{IntoDiagnostic, miette};
use rustyx::RustyX;
use serde_json::json;
use tracing::{error, info, warn};
use tracing_subscriber::fmt;

use crate::schemas::{ClientState, CluManSchema, ControllerState, Mode, ServerState, Tasks};

mod schemas;

const PORT: u16 = 9999;

// ── Entry point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> miette::Result<()> {
    fmt().init();

    // Dispatch on argv[0], just like actman.
    // The binary is expected to be invoked (or symlinked) as `client`,
    // `server`, or `controller` / `cluman`.
    let argv0 = args().next().unwrap_or_default();
    let mode = Mode::from_str(
        Path::new(&argv0)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .as_ref(),
    )?;

    let cmdline = CmdLineOptions::new()?;

    match mode {
        Mode::Controller => run_controller(&cmdline).await,
        Mode::Server => run_server(&cmdline).await,
        Mode::Client => run_client(&cmdline).await,
    }
}

// ── Controller ────────────────────────────────────────────────────────────────
//
// Kernel cmdline keys (all optional):
//   own_ip=<ipv4>
//
// Exposes:
//   POST /register-server  — called by servers on start-up
//   GET  /tasks            — polled by servers; returns the current task queue
//   POST /tasks            — called by CI / an admin to enqueue a new task
//   GET  /servers          — introspection: list of registered server IPs

async fn run_controller(cmdline: &CmdLineOptions) -> miette::Result<()> {
    let own_ip = cmdline
        .opts()
        .get("own_ip")
        .and_then(|s| s.parse::<Ipv4Addr>().ok());

    if let Some(ip) = own_ip {
        info!(%ip, "Controller starting");
    } else {
        info!("Controller starting");
    }

    let state = ControllerState::new();
    let server = RustyX::new();

    // POST /register-server
    // Body: CluManSchema with ips = Right((ip, Mode::Server))
    let st = state.clone();
    server.post("/register-server", move |req, res| {
        let st = st.clone();
        async move {
            match req.json::<CluManSchema>() {
                Ok(schema) if schema.peer().map_or(false, |(_, m)| m == Mode::Server) => {
                    let (ip, _) = schema.peer().unwrap();
                    st.register_server(ip);
                    info!(%ip, "Server registered");
                    res.created(json!({ "response": "Successfully registered" }))
                }
                _ => {
                    res.bad_request("Invalid request. Only servers can register with controllers.")
                }
            }
        }
    });

    // GET /tasks — servers poll this to receive work
    let st = state.clone();
    server.get("/tasks", move |_req, res| {
        let st = st.clone();
        async move {
            let tasks = st.snapshot_tasks();
            res.json(serde_json::to_value(&tasks).unwrap_or_default())
        }
    });

    // POST /tasks — CI / admin pushes a compose-file path to execute
    // Body: { "task": "/path/to/docker-compose.yml" }
    let st = state.clone();
    server.post("/tasks", move |req, res| {
        let st = st.clone();
        async move {
            match req.json::<serde_json::Value>() {
                Ok(body) => match body.get("task").and_then(|t| t.as_str()) {
                    Some(task) => {
                        st.add_task(task.to_string());
                        info!(task, "Task queued");
                        res.created(json!({ "response": "Task queued" }))
                    }
                    None => res.bad_request("Missing 'task' field"),
                },
                Err(e) => res.bad_request(&e.to_string()),
            }
        }
    });

    // GET /servers — introspection
    let st = state.clone();
    server.get("/servers", move |_req, res| {
        let st = st.clone();
        async move {
            let addrs: Vec<String> = st.server_addrs().iter().map(|a| a.to_string()).collect();
            res.json(json!({ "servers": addrs }))
        }
    });

    info!(port = PORT, "Controller listening");
    server.listen(PORT).await.into_diagnostic()
}

// ── Server ────────────────────────────────────────────────────────────────────
//
// Kernel cmdline keys:
//   contrl_urls=http://1.2.3.4:9999,http://...   (required — comma-separated)
//   own_ip=<ipv4>                                 (used in registration payload)
//
// On start-up:
//   • Registers itself with every listed controller.
//   • Spawns a background thread that polls each controller for new tasks
//     every 20 s and enqueues them locally.
//
// Exposes:
//   POST /api/register-client  — called by clients on start-up
//   GET  /task                 — clients claim the next pending task
//   GET  /clients              — introspection: list of registered client IPs

async fn run_server(cmdline: &CmdLineOptions) -> miette::Result<()> {
    let controller_urls: Vec<String> = cmdline
        .opts()
        .get("contrl_urls")
        .map(|s| s.split(',').map(str::to_string).collect())
        .unwrap_or_default();

    if controller_urls.is_empty() {
        return Err(miette!(
            "No controller URLs in /proc/cmdline — add contrl_urls=http://HOST:PORT,..."
        ));
    }

    let own_ip: Ipv4Addr = cmdline
        .opts()
        .get("own_ip")
        .and_then(|s| s.parse().ok())
        .unwrap_or(Ipv4Addr::LOCALHOST);

    info!(%own_ip, "Server starting");

    let state = ServerState::new();
    let server = RustyX::new();

    // ── Register with every controller ────────────────────────────────────────
    let payload = serde_json::to_string(&CluManSchema::registration(own_ip, Mode::Server))
        .into_diagnostic()?;

    for url in &controller_urls {
        match minreq::post(format!("{url}/register-server"))
            .with_header("Content-Type", "application/json")
            .with_body(payload.clone())
            .send()
        {
            Ok(resp) => info!(url, status = resp.status_code, "Registered with controller"),
            Err(e) => warn!(url, error = %e, "Failed to register with controller"),
        }
    }

    // ── Background: poll controllers for new tasks every 20 s ─────────────────
    // minreq is synchronous, so we keep this in a plain OS thread to avoid
    // blocking the tokio executor.
    let st = state.clone();
    thread::spawn(move || {
        loop {
            for url in &controller_urls {
                match minreq::get(format!("{url}/tasks")).send() {
                    Ok(resp) if resp.status_code == 200 => {
                        if let Ok(body) = resp.as_str() {
                            match serde_json::from_str::<Tasks>(body) {
                                Ok(tasks) if !tasks.is_empty() => {
                                    info!(
                                        count = tasks.len(),
                                        url, "Received tasks from controller"
                                    );
                                    st.enqueue_tasks(tasks);
                                }
                                Ok(_) => {} // empty list — nothing to do
                                Err(e) => warn!(url, error = %e, "Could not parse task list"),
                            }
                        }
                    }
                    Ok(resp) => warn!(
                        url,
                        status = resp.status_code,
                        "Unexpected status polling tasks"
                    ),
                    Err(e) => warn!(url, error = %e, "Failed to poll controller"),
                }
            }
            thread::sleep(Duration::from_secs(20));
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

    // ── GET /task — a client claims the next pending task ─────────────────────
    let st = state.clone();
    server.get("/task", move |_req, res| {
        let st = st.clone();
        async move {
            match st.claim_task() {
                Some(task) => {
                    info!(task, "Task dispatched to client");
                    res.json(json!({ "task": task }))
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

    info!(port = PORT, "Server listening");
    server.listen(PORT).await.into_diagnostic()
}

// ── Client ────────────────────────────────────────────────────────────────────
//
// Kernel cmdline keys:
//   server_url=http://HOST:PORT   (required)
//   own_ip=<ipv4>                 (used in registration payload)
//
// On start-up:
//   • Registers itself with the server.
//   • Spawns a background thread that polls the server's GET /task every 10 s.
//     When a task arrives it is executed synchronously via
//     `docker compose -f <path> up -d --remove-orphans`.
//
// Exposes:
//   GET /status   — simple health-check used by the server / monitoring

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
    let st = state.clone();
    thread::spawn(move || {
        loop {
            match minreq::get(format!("{}/task", st.server_url)).send() {
                Ok(resp) if resp.status_code == 200 => {
                    if let Ok(body) = resp.as_str() {
                        if let Ok(value) = serde_json::from_str::<serde_json::Value>(body) {
                            if let Some(task) = value.get("task").and_then(|t| t.as_str()) {
                                if !task.is_empty() {
                                    info!(task, "Received task from server");
                                    execute_compose(task);
                                }
                            }
                        }
                    }
                }
                Ok(resp) if resp.status_code == 204 => {} // no tasks yet
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

/// Run `docker compose -f <compose_file> up -d --remove-orphans`.
///
/// Executed synchronously inside the polling thread so that at most one
/// compose project is started at a time per client node.
fn execute_compose(compose_file: &str) {
    match Command::new("docker")
        .args([
            "compose",
            "-f",
            compose_file,
            "up",
            "-d",
            "--remove-orphans",
        ])
        .output()
    {
        Ok(out) if out.status.success() => {
            info!(compose_file, "docker compose up succeeded");
        }
        Ok(out) => {
            error!(
                compose_file,
                exit_code = out.status.code().unwrap_or(-1),
                stderr    = %String::from_utf8_lossy(&out.stderr),
                "docker compose up failed",
            );
        }
        Err(e) => {
            error!(compose_file, error = %e, "Failed to spawn docker compose");
        }
    }
}
