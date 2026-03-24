//! Server personality of the `cluman` binary.
//!
//! When `cluman` is launched in server mode it runs [`run_server`], which owns
//! the shared [`ServerState`] and exposes an HTTP API that lets controllers
//! push work and lets clients claim that work.

use actman::cmdline::CmdLineOptions;
use miette::IntoDiagnostic;
use rustix::system::reboot;
use rustyx::RustyX;
use serde_json::json;
use std::{net::Ipv4Addr, thread::spawn};
use tracing::info;

use crate::{
    PORT,
    schemas::{CluManSchema, Mode, ServerState, Task},
};

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

/// Start the `cluman` HTTP server and block until it shuts down.
///
/// # Startup
///
/// `own_ip` is resolved by looking for an `own_ip=<ipv4>` key in the kernel
/// command line (via [`CmdLineOptions`]).  If the key is absent or cannot be
/// parsed as a valid [`Ipv4Addr`] it falls back to `127.0.0.1`.
///
/// # Routes (all bound to [`PORT`] — 9999)
///
/// | Method | Path | Description |
/// |--------|------|-------------|
/// | `POST` | `/api/push-task` | Receives a [`Task`] JSON body from a controller and appends it to the pending-task queue. |
/// | `POST` | `/api/register-client` | Accepts a [`CluManSchema`] whose peer is a [`Mode::Client`]; records the client IP in [`ServerState`]. |
/// | `GET`  | `/task` | Atomically claims the next pending [`Task`] for a polling client.  Returns `204 No Content` when the queue is empty. |
/// | `GET`  | `/clients` | Returns the list of currently registered client IP addresses as a JSON array. |
/// | `GET`  | `/pending` | Returns the number of tasks currently waiting in the queue as JSON. |
/// | `GET`  | `/reboot` | Reboots the machine.  Requires a valid [`CluManSchema`] body; returns `400` otherwise. |
/// | `GET`  | `/poweroff` | Powers off the machine.  Requires a valid [`CluManSchema`] body; returns `400` otherwise. |
///
/// # Errors
///
/// Returns a [`miette::Result`] that propagates any error raised by
/// [`RustyX::listen`] (e.g. the port is already in use).
pub(crate) async fn run_server(cmdline: &CmdLineOptions) -> miette::Result<()> {
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
    server.get("/reboot", async move |req, res| {
        match req.json::<CluManSchema>() {
            Ok(_) => {
                spawn(move || reboot(rustix::system::RebootCommand::Restart));
                res.status(200)
            }
            Err(_) => res.status(400),
        }
    });
    server.get("/poweroff", async move |req, res| {
        match req.json::<CluManSchema>() {
            Ok(_) => {
                spawn(move || reboot(rustix::system::RebootCommand::PowerOff));
                res.status(200)
            }
            Err(_) => res.status(400),
        }
    });
    // ── POST /api/register-client ─────────────────────────────────────────────
    let st = state.clone();
    server.post("/api/register-client", move |req, res| {
        let st = st.clone();
        async move {
            match req.json::<CluManSchema>() {
                Ok(schema) if schema.peer().is_some_and(|(_, m)| m == Mode::Client) => {
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
            let addrs: Vec<String> = st
                .client_addrs()
                .iter()
                .map(|a: &std::net::Ipv4Addr| a.to_string())
                .collect();
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
