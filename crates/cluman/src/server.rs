use std::net::Ipv4Addr;

use actman::cmdline::CmdLineOptions;
use miette::IntoDiagnostic;
use rustyx::RustyX;
use serde_json::json;
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
