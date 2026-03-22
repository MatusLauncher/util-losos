//! Integration tests for the cluman server HTTP routes.
//!
//! ## Required dev-dependencies
//!
//! Add to `cluman/Cargo.toml` `[dev-dependencies]`:
//! ```toml
//! minreq     = "2.14.1"
//! rustyx     = "0.2.0"
//! serde_json = "1.0.149"
//! ```
//!
//! ## Design
//!
//! Each test creates an isolated [`TestServer`] backed by a fresh
//! [`ServerState`].  The server is spun up in an OS thread (using a
//! dedicated multi-thread Tokio runtime) on an ephemeral loopback port
//! obtained from the OS, so tests are safe to run fully in parallel without
//! port conflicts.
//!
//! [`build_server`] mirrors the route wiring in `server.rs` using only the
//! public `cluman::schemas` API.  This means the tests double as contract
//! tests: any change to `ServerState`'s semantics breaks them immediately.

use std::net::{Ipv4Addr, TcpListener};
use std::thread;
use std::time::Duration;

use cluman::schemas::{CluManSchema, Mode, ServerState, Task};
use serde_json::{Value, json};

// ── Port allocation ───────────────────────────────────────────────────────────

/// Bind to port 0, let the OS assign a free port, then release the socket.
/// `RustyX` binds independently a moment later; the tiny race is acceptable
/// in tests.
fn alloc_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .expect("bind to :0")
        .local_addr()
        .expect("local_addr")
        .port()
}

// ── TestServer ────────────────────────────────────────────────────────────────

/// A live HTTP server with direct access to its [`ServerState`] for
/// white-box assertions.
struct TestServer {
    /// Direct handle — lets tests assert state without going through HTTP.
    state: ServerState,
    base_url: String,
}

impl TestServer {
    /// Start a server on a fresh ephemeral port and wait until it accepts
    /// connections (up to ≈ 2 s).
    fn new() -> Self {
        let port = alloc_port();
        let base_url = format!("http://127.0.0.1:{port}");
        let state = ServerState::new();

        let st = state.clone();
        thread::spawn(move || {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime")
                .block_on(async move {
                    build_server(st).listen(port).await.expect("server listen");
                });
        });

        // Poll /pending until the server is ready (max ≈ 2 s / 40 × 50 ms).
        let probe = format!("{base_url}/pending");
        for _ in 0..40 {
            if minreq::get(&probe).send().is_ok() {
                return Self { state, base_url };
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("Test server on port {port} did not become ready within 2 s");
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    // ── Route helpers ─────────────────────────────────────────────────────────

    fn push_task_req(&self, task: &Task) -> minreq::Response {
        minreq::post(self.url("/api/push-task"))
            .with_header("Content-Type", "application/json")
            .with_body(serde_json::to_string(task).unwrap())
            .send()
            .expect("POST /api/push-task")
    }

    fn register_client_req(&self, ip: Ipv4Addr) -> minreq::Response {
        self.register_with_mode_req(ip, Mode::Client)
    }

    fn register_with_mode_req(&self, ip: Ipv4Addr, mode: Mode) -> minreq::Response {
        let body = serde_json::to_string(&CluManSchema::registration(ip, mode)).unwrap();
        minreq::post(self.url("/api/register-client"))
            .with_header("Content-Type", "application/json")
            .with_body(body)
            .send()
            .expect("POST /api/register-client")
    }

    fn claim_task_req(&self) -> minreq::Response {
        minreq::get(self.url("/task")).send().expect("GET /task")
    }

    fn clients_req(&self) -> minreq::Response {
        minreq::get(self.url("/clients"))
            .send()
            .expect("GET /clients")
    }

    fn pending_req(&self) -> minreq::Response {
        minreq::get(self.url("/pending"))
            .send()
            .expect("GET /pending")
    }
}

// ── Server builder ────────────────────────────────────────────────────────────

/// Build a `RustyX` app with the same five routes as `server.rs`.
///
/// Using only the public `ServerState` API means this is automatically
/// kept honest — it can't call anything the real server can't call.
fn build_server(state: ServerState) -> rustyx::RustyX {
    let app = rustyx::RustyX::new();

    // POST /api/push-task ─────────────────────────────────────────────────────
    let st = state.clone();
    app.post("/api/push-task", move |req, res| {
        let st = st.clone();
        async move {
            match req.json::<Task>() {
                Ok(task) => {
                    st.push_task(task);
                    res.created(json!({ "response": "Task queued" }))
                }
                Err(e) => res.bad_request(&e.to_string()),
            }
        }
    });

    // POST /api/register-client ───────────────────────────────────────────────
    let st = state.clone();
    app.post("/api/register-client", move |req, res| {
        let st = st.clone();
        async move {
            match req.json::<CluManSchema>() {
                Ok(schema) if schema.peer().is_some_and(|(_, m)| m == Mode::Client) => {
                    let (ip, _) = schema.peer().unwrap();
                    st.register_client(ip);
                    res.created(json!({ "response": "Successfully registered" }))
                }
                _ => res.bad_request("Only clients can register with servers."),
            }
        }
    });

    // GET /task ───────────────────────────────────────────────────────────────
    let st = state.clone();
    app.get("/task", move |_req, res| {
        let st = st.clone();
        async move {
            match st.claim_task() {
                Some(task) => res.json(serde_json::to_value(&task).unwrap_or_default()),
                None => res.no_content(),
            }
        }
    });

    // GET /clients ────────────────────────────────────────────────────────────
    let st = state.clone();
    app.get("/clients", move |_req, res| {
        let st = st.clone();
        async move {
            let addrs: Vec<String> = st
                .client_addrs()
                .iter()
                .map(|a: &Ipv4Addr| a.to_string())
                .collect();
            res.json(json!({ "clients": addrs }))
        }
    });

    // GET /pending ────────────────────────────────────────────────────────────
    let st = state.clone();
    app.get("/pending", move |_req, res| {
        let st = st.clone();
        async move { res.json(json!({ "pending": st.pending_count() })) }
    });

    app
}

// ── Response helpers ──────────────────────────────────────────────────────────

fn json_body(resp: &minreq::Response) -> Value {
    serde_json::from_str(resp.as_str().expect("response body as str"))
        .expect("response body is valid JSON")
}

fn pending_count(srv: &TestServer) -> u64 {
    json_body(&srv.pending_req())["pending"]
        .as_u64()
        .expect("pending field is u64")
}

fn client_list(srv: &TestServer) -> Vec<String> {
    json_body(&srv.clients_req())["clients"]
        .as_array()
        .expect("clients field is array")
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect()
}

// ── GET /pending ──────────────────────────────────────────────────────────────

#[test]
fn pending_is_zero_on_fresh_server() {
    let srv = TestServer::new();
    let resp = srv.pending_req();
    assert_eq!(resp.status_code, 200);
    assert_eq!(pending_count(&srv), 0);
}

#[test]
fn pending_increments_with_each_push() {
    let srv = TestServer::new();
    srv.push_task_req(&Task::new("a.yml", ""));
    assert_eq!(pending_count(&srv), 1);
    srv.push_task_req(&Task::new("b.yml", ""));
    assert_eq!(pending_count(&srv), 2);
    srv.push_task_req(&Task::new("c.yml", ""));
    assert_eq!(pending_count(&srv), 3);
}

#[test]
fn pending_decrements_after_each_claim() {
    let srv = TestServer::new();
    srv.push_task_req(&Task::new("a.yml", ""));
    srv.push_task_req(&Task::new("b.yml", ""));
    assert_eq!(pending_count(&srv), 2);
    srv.claim_task_req();
    assert_eq!(pending_count(&srv), 1);
    srv.claim_task_req();
    assert_eq!(pending_count(&srv), 0);
}

#[test]
fn pending_returns_zero_after_full_drain() {
    let srv = TestServer::new();
    for i in 0..5u8 {
        srv.push_task_req(&Task::new(format!("{i}.yml"), ""));
    }
    for _ in 0..5 {
        srv.claim_task_req();
    }
    assert_eq!(pending_count(&srv), 0);
}

// ── POST /api/push-task ───────────────────────────────────────────────────────

#[test]
fn push_task_returns_201_for_valid_task() {
    let srv = TestServer::new();
    let resp = srv.push_task_req(&Task::new("compose.yml", "version: '3'"));
    assert_eq!(resp.status_code, 201);
}

#[test]
fn push_task_response_contains_acknowledgement_field() {
    let srv = TestServer::new();
    let resp = srv.push_task_req(&Task::new("compose.yml", ""));
    let body = json_body(&resp);
    assert!(
        body["response"].as_str().is_some(),
        "expected a 'response' string in body"
    );
}

#[test]
fn push_task_returns_400_for_invalid_json() {
    let srv = TestServer::new();
    let resp = minreq::post(srv.url("/api/push-task"))
        .with_header("Content-Type", "application/json")
        .with_body("not json at all")
        .send()
        .expect("request");
    assert_eq!(resp.status_code, 400);
}

#[test]
fn push_task_returns_400_when_content_field_missing() {
    let srv = TestServer::new();
    // Only `filename` provided; `content` is required by Task.
    let resp = minreq::post(srv.url("/api/push-task"))
        .with_header("Content-Type", "application/json")
        .with_body(r#"{"filename":"only.yml"}"#)
        .send()
        .expect("request");
    assert_eq!(resp.status_code, 400);
}

#[test]
fn push_task_returns_400_when_filename_field_missing() {
    let srv = TestServer::new();
    let resp = minreq::post(srv.url("/api/push-task"))
        .with_header("Content-Type", "application/json")
        .with_body(r#"{"content":"data"}"#)
        .send()
        .expect("request");
    assert_eq!(resp.status_code, 400);
}

#[test]
fn push_task_adds_to_state_pending_count() {
    let srv = TestServer::new();
    assert_eq!(srv.state.pending_count(), 0);
    srv.push_task_req(&Task::new("x.yml", ""));
    // Assert via the shared state handle, not via HTTP, to avoid coupling.
    assert_eq!(srv.state.pending_count(), 1);
}

// ── GET /task ─────────────────────────────────────────────────────────────────

#[test]
fn get_task_returns_204_when_queue_is_empty() {
    let srv = TestServer::new();
    assert_eq!(srv.claim_task_req().status_code, 204);
}

#[test]
fn get_task_returns_200_when_task_is_available() {
    let srv = TestServer::new();
    srv.push_task_req(&Task::new("web.yml", "image: nginx"));
    assert_eq!(srv.claim_task_req().status_code, 200);
}

#[test]
fn get_task_response_contains_filename_and_content() {
    let srv = TestServer::new();
    srv.push_task_req(&Task::new("web.yml", "image: nginx"));
    let body = json_body(&srv.claim_task_req());
    assert_eq!(body["filename"].as_str().unwrap(), "web.yml");
    assert_eq!(body["content"].as_str().unwrap(), "image: nginx");
}

#[test]
fn get_task_preserves_multiline_content_verbatim() {
    let srv = TestServer::new();
    let content = "services:\n  web:\n    image: nginx:latest\n    ports:\n      - \"80:80\"\n";
    srv.push_task_req(&Task::new("nginx.yml", content));
    let body = json_body(&srv.claim_task_req());
    assert_eq!(body["content"].as_str().unwrap(), content);
}

#[test]
fn get_task_is_fifo() {
    let srv = TestServer::new();
    srv.push_task_req(&Task::new("first.yml", ""));
    srv.push_task_req(&Task::new("second.yml", ""));
    srv.push_task_req(&Task::new("third.yml", ""));

    let b1 = json_body(&srv.claim_task_req());
    let b2 = json_body(&srv.claim_task_req());
    let b3 = json_body(&srv.claim_task_req());

    assert_eq!(b1["filename"].as_str().unwrap(), "first.yml");
    assert_eq!(b2["filename"].as_str().unwrap(), "second.yml");
    assert_eq!(b3["filename"].as_str().unwrap(), "third.yml");
}

#[test]
fn get_task_returns_204_once_queue_is_drained() {
    let srv = TestServer::new();
    srv.push_task_req(&Task::new("only.yml", ""));
    srv.claim_task_req(); // drain the one task
    assert_eq!(srv.claim_task_req().status_code, 204);
}

#[test]
fn get_task_removes_entry_from_shared_state() {
    let srv = TestServer::new();
    srv.push_task_req(&Task::new("a.yml", ""));
    assert_eq!(srv.state.pending_count(), 1);
    srv.claim_task_req();
    assert_eq!(srv.state.pending_count(), 0);
}

// ── POST /api/register-client ─────────────────────────────────────────────────

#[test]
fn register_client_returns_201_for_client_mode() {
    let srv = TestServer::new();
    let resp = srv.register_client_req("10.0.0.1".parse().unwrap());
    assert_eq!(resp.status_code, 201);
}

#[test]
fn register_client_response_contains_acknowledgement_field() {
    let srv = TestServer::new();
    let resp = srv.register_client_req("10.0.0.1".parse().unwrap());
    let body = json_body(&resp);
    assert!(body["response"].as_str().is_some());
}

#[test]
fn register_client_adds_ip_to_state() {
    let srv = TestServer::new();
    let ip: Ipv4Addr = "10.0.0.5".parse().unwrap();
    srv.register_client_req(ip);
    assert!(srv.state.client_addrs().contains(&ip));
}

#[test]
fn register_client_rejects_server_mode_with_400() {
    let srv = TestServer::new();
    let resp = srv.register_with_mode_req("10.0.0.2".parse().unwrap(), Mode::Server);
    assert_eq!(resp.status_code, 400);
}

#[test]
fn register_client_rejects_controller_mode_with_400() {
    let srv = TestServer::new();
    let resp = srv.register_with_mode_req("10.0.0.3".parse().unwrap(), Mode::Controller);
    assert_eq!(resp.status_code, 400);
}

#[test]
fn register_client_does_not_add_rejected_server_to_state() {
    let srv = TestServer::new();
    let ip: Ipv4Addr = "10.0.0.4".parse().unwrap();
    srv.register_with_mode_req(ip, Mode::Server);
    assert!(!srv.state.client_addrs().contains(&ip));
}

#[test]
fn register_client_returns_400_for_invalid_json() {
    let srv = TestServer::new();
    let resp = minreq::post(srv.url("/api/register-client"))
        .with_header("Content-Type", "application/json")
        .with_body("{bad json}")
        .send()
        .expect("request");
    assert_eq!(resp.status_code, 400);
}

#[test]
fn register_same_client_twice_is_idempotent_in_state() {
    let srv = TestServer::new();
    let ip: Ipv4Addr = "10.0.0.9".parse().unwrap();
    srv.register_client_req(ip);
    srv.register_client_req(ip);
    // HashMap insert overwrites — state must still contain exactly one entry.
    assert_eq!(srv.state.client_addrs().len(), 1);
}

#[test]
fn register_same_client_twice_is_idempotent_in_http_response() {
    let srv = TestServer::new();
    let ip: Ipv4Addr = "10.0.0.9".parse().unwrap();
    srv.register_client_req(ip);
    srv.register_client_req(ip);
    let clients = client_list(&srv);
    let occurrences = clients.iter().filter(|a| a.as_str() == "10.0.0.9").count();
    assert_eq!(occurrences, 1);
}

// ── GET /clients ──────────────────────────────────────────────────────────────

#[test]
fn clients_returns_200_initially() {
    let srv = TestServer::new();
    assert_eq!(srv.clients_req().status_code, 200);
}

#[test]
fn clients_returns_empty_array_on_fresh_server() {
    let srv = TestServer::new();
    assert!(client_list(&srv).is_empty());
}

#[test]
fn clients_shows_registered_ip() {
    let srv = TestServer::new();
    let ip: Ipv4Addr = "10.0.0.20".parse().unwrap();
    srv.register_client_req(ip);
    assert!(client_list(&srv).contains(&"10.0.0.20".to_string()));
}

#[test]
fn clients_shows_all_registered_ips() {
    let srv = TestServer::new();
    let ips = ["10.0.1.1", "10.0.1.2", "10.0.1.3"];
    for ip_str in &ips {
        srv.register_client_req(ip_str.parse().unwrap());
    }
    let listed = client_list(&srv);
    assert_eq!(listed.len(), 3);
    for ip_str in &ips {
        assert!(listed.contains(&ip_str.to_string()));
    }
}

#[test]
fn clients_count_matches_state_client_addrs_len() {
    let srv = TestServer::new();
    for i in 1..=4u8 {
        srv.register_client_req(Ipv4Addr::new(10, 0, 0, i));
    }
    assert_eq!(client_list(&srv).len(), srv.state.client_addrs().len());
}

// ── End-to-end flows ──────────────────────────────────────────────────────────

#[test]
fn full_cluster_flow_register_push_claim() {
    let srv = TestServer::new();

    // 1. Two clients register.
    let client1: Ipv4Addr = "10.10.0.1".parse().unwrap();
    let client2: Ipv4Addr = "10.10.0.2".parse().unwrap();
    assert_eq!(srv.register_client_req(client1).status_code, 201);
    assert_eq!(srv.register_client_req(client2).status_code, 201);
    assert_eq!(client_list(&srv).len(), 2);

    // 2. Controller pushes two tasks.
    assert_eq!(
        srv.push_task_req(&Task::new("db.yml", "image: postgres"))
            .status_code,
        201
    );
    assert_eq!(
        srv.push_task_req(&Task::new("web.yml", "image: nginx"))
            .status_code,
        201
    );
    assert_eq!(pending_count(&srv), 2);

    // 3. Clients each claim one task; FIFO order is preserved.
    let t1 = json_body(&srv.claim_task_req());
    let t2 = json_body(&srv.claim_task_req());
    assert_eq!(t1["filename"].as_str().unwrap(), "db.yml");
    assert_eq!(t2["filename"].as_str().unwrap(), "web.yml");

    // 4. Queue is now empty.
    assert_eq!(pending_count(&srv), 0);
    assert_eq!(srv.claim_task_req().status_code, 204);
}

#[test]
fn controller_pushes_compose_file_content_via_http() {
    let srv = TestServer::new();

    // Simulate a controller reading a compose file from disk and sending it.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("stack.yml");
    let compose = "services:\n  app:\n    image: myapp:latest\n    ports:\n      - 8080:8080\n";
    std::fs::write(&path, compose).unwrap();

    let filename = path.file_name().unwrap().to_string_lossy().into_owned();
    let content = std::fs::read_to_string(&path).unwrap();

    assert_eq!(
        srv.push_task_req(&Task::new(filename.clone(), content.clone()))
            .status_code,
        201
    );
    assert_eq!(pending_count(&srv), 1);

    // Client claims and verifies byte-for-byte fidelity.
    let body = json_body(&srv.claim_task_req());
    assert_eq!(body["filename"].as_str().unwrap(), filename);
    assert_eq!(body["content"].as_str().unwrap(), content);
}

#[test]
fn interleaved_push_and_claim_preserves_global_fifo_order() {
    let srv = TestServer::new();

    // Push 3 tasks.
    for i in 0..3u8 {
        srv.push_task_req(&Task::new(format!("{i}.yml"), ""));
    }

    // Claim 1 — must be the very first pushed.
    let first = json_body(&srv.claim_task_req());
    assert_eq!(first["filename"].as_str().unwrap(), "0.yml");

    // Push 2 more tasks while the queue still has entries.
    for i in 3..5u8 {
        srv.push_task_req(&Task::new(format!("{i}.yml"), ""));
    }

    // Drain the remaining 4 and check ordering.
    let filenames = ["1.yml", "2.yml", "3.yml", "4.yml"];
    for expected in filenames {
        let body = json_body(&srv.claim_task_req());
        assert_eq!(body["filename"].as_str().unwrap(), expected);
    }

    // Queue must be empty after draining all tasks.
    assert_eq!(srv.claim_task_req().status_code, 204);
}

#[test]
fn multiple_clients_each_receive_distinct_tasks() {
    let srv = TestServer::new();

    let tasks = ["alpha.yml", "beta.yml", "gamma.yml"];
    for name in &tasks {
        srv.push_task_req(&Task::new(*name, ""));
    }

    // Three separate "clients" each claim one task.
    let claimed: Vec<String> = (0..3)
        .map(|_| {
            json_body(&srv.claim_task_req())["filename"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();

    // Every task must have been claimed exactly once.
    for name in &tasks {
        assert_eq!(claimed.iter().filter(|f| f.as_str() == *name).count(), 1);
    }

    // Nothing left.
    assert_eq!(srv.claim_task_req().status_code, 204);
}
